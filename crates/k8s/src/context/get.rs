use agent_cli_core::{Ctx, Failure, Output, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, Target, finished, items, non_empty};

#[derive(clap::Args)]
pub struct ContextGetArgs {
    /// The [[k8s.scope]] name (or its kube context); defaults to the only one
    cluster: Option<String>,
    /// The same, as a flag
    #[arg(long = "cluster")]
    cluster_flag: Option<String>,
}

/// A cluster as a whole: what is wrong with it before any one object.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ContextDetail {
    /// The scope: what --cluster takes.
    name: String,
    context: String,
    /// The API server's version: v1.30.4.
    version: Option<String>,
    /// The API server's readiness checks that fail (`/readyz?verbose`);
    /// none is healthy.
    failing: Vec<String>,
    /// kube-system's kube-dns service IP, the resolver every pod's
    /// resolv.conf names.
    dns_ip: Option<String>,
    /// The network plugin, from kube-system's daemonsets: azure-cni,
    /// cilium, calico, flannel.
    cni: Option<String>,
    nodes: usize,
}

fn context_get(ctx: &Ctx, args: ContextGetArgs) -> Result<ContextDetail> {
    let cluster = match (args.cluster, args.cluster_flag) {
        (Some(held), Some(flag)) if held != flag => {
            return Err(
                Failure::usage(format!("CLUSTER says {held}, and --cluster says {flag}")).into(),
            );
        }
        (held, flag) => held.or(flag),
    };
    let target = At {
        cluster,
        namespace: None,
    }
    .cluster_wide(ctx)?;
    let version = target.json(ctx, &["version", "-o", "json"])?;
    let failing = failing(ctx.read(target.kubectl(&["get", "--raw", "/readyz?verbose"]))?)
        .unwrap_or_else(|error| {
            ctx.note(format!("[health checks not read: {error:#}]"));
            Vec::new()
        });
    // kube-system is read whatever namespaces the scope names: it is the
    // cluster's own, and what this command is about.
    let system = Target {
        namespace: Some("kube-system".to_owned()),
        cluster_scoped: false,
        ..target
    };
    let listed = system.json(ctx, &["get", "nodes,services,daemonsets", "-o", "json"])?;
    let of = |kind: &'static str| items(&listed).filter(move |item| item["kind"] == kind);
    Ok(ContextDetail {
        version: non_empty(&version["serverVersion"]["gitVersion"]).map(str::to_owned),
        failing,
        dns_ip: of("Service")
            .find(|item| item["metadata"]["name"] == "kube-dns")
            .and_then(|item| non_empty(&item["spec"]["clusterIP"]))
            .map(str::to_owned),
        cni: cni(of("DaemonSet").filter_map(|item| item["metadata"]["name"].as_str())),
        nodes: of("Node").count(),
        name: system.scope,
        context: system.context,
    })
}

/// The checks `/readyz?verbose` marks `[-]`. A failing check makes it a
/// 500, which kubectl reports with the body quoted and `\n` escaped, so the
/// lines are read from either stream.
// VERIFY(work): the 500's wording on a real AKS API server.
fn failing(output: Output) -> Result<Vec<String>> {
    let text = format!("{}\n{}", output.stdout, output.stderr).replace("\\n", "\n");
    let failing: Vec<String> = text
        .lines()
        .filter_map(|line| line.split_once("[-]"))
        .map(|(_, check)| check.trim().to_owned())
        .collect();
    if failing.is_empty() && !output.status.success() {
        finished(output)?;
    }
    Ok(failing)
}

/// The network plugin a cluster runs, by its daemonset in kube-system.
// ponytail: by daemonset name only; Azure CNI overlay and node subnet, and
// kubenet (no daemonset of its own), are not told apart. `aks cluster get`'s
// network profile says it exactly.
fn cni<'a>(daemonsets: impl Iterator<Item = &'a str>) -> Option<String> {
    const PLUGINS: [(&str, &str); 4] = [
        ("cilium", "cilium"),
        ("calico-node", "calico"),
        ("azure-cns", "azure-cni"),
        ("kube-flannel", "flannel"),
    ];
    let names: Vec<&str> = daemonsets.collect();
    PLUGINS
        .iter()
        .find(|(prefix, _)| names.iter().any(|name| name.starts_with(prefix)))
        .map(|(_, plugin)| (*plugin).to_owned())
}

command! {
    pub CONTEXT_GET = ["k8s", "context", "get"], Read,
    "Check a cluster: server version, failing readiness checks, DNS address, CNI",
    keywords: ["kubernetes", "coredns", "network", "plugin", "etcd", "readyz"],
    example: "k8s context get qa",
    run: context_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn a_cluster_reports_its_version_failing_checks_dns_ip_cni_and_nodes() {
        let outcome = k8s(&["k8s", "context", "get", "qa"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"name": "qa", "context": "aks-qa", "version": "v1.29.7",
                "failing": ["etcd failed: reason withheld"], "dns_ip": "10.0.0.10", "cni": "cilium", "nodes": 2})
        );
        let outcome = k8s(&["k8s", "context", "get", "--cluster", "prod"]);
        assert_eq!(
            outcome.json(),
            json!({"name": "prod", "context": "aks-prod", "version": "v1.30.0", "nodes": 0}),
            "{outcome:?}"
        );
        let outcome = k8s(&["k8s", "context", "get", "qa", "--cluster", "prod"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        let outcome = k8s(&["k8s", "context", "get"]);
        assert_eq!(outcome.code, 2, "three scopes, none named: {outcome:?}");
    }
}
