//! `k8s object get`: one object of any kind, `kubectl get KIND NAME -o
//! json`, trimmed of the bookkeeping. Whether the kind is namespaced (from
//! `k8s kind`) decides how its ref reads.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::kind::kind_named;
use crate::kubectl::At;
use crate::shape::trim;

#[derive(clap::Args)]
pub struct ObjectGetArgs {
    /// The kind, as `k8s kind list` names it: certificates, cert, node
    kind: String,
    /// The object: its id from object list (cluster/namespace/name, or
    /// cluster/name for a cluster-scoped kind), namespace/name, or name
    object: String,
    #[command(flatten)]
    at: At,
    /// The manifest as kubectl's YAML text instead
    #[arg(long)]
    yaml: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ObjectDetail {
    id: String,
    /// The object as the API server holds it, spec and status, without
    /// managedFields or the last-applied annotation.
    manifest: Option<Value>,
    /// The manifest, with --yaml.
    text: Option<String>,
}

fn object_get(ctx: &Ctx, args: ObjectGetArgs) -> Result<ObjectDetail> {
    let probe = args.at.cluster_of(ctx, &args.object)?;
    let kind = kind_named(ctx, &probe, &args.kind)?;
    let (target, name) = if kind.namespaced {
        args.at.named(ctx, &args.object)?
    } else {
        args.at.named_cluster(ctx, &args.object)?
    };
    let qualified = kind.qualified();
    if args.yaml {
        // ponytail: kubectl's own YAML: managedFields hidden (its default),
        // the last-applied annotation kept; trimming it needs a YAML writer.
        return Ok(ObjectDetail {
            id: target.id(&json!({"metadata": {"name": name}})),
            manifest: None,
            text: Some(target.read(ctx, &["get", &qualified, &name, "-o", "yaml"])?),
        });
    }
    let item = target.json(ctx, &["get", &qualified, &name, "-o", "json"])?;
    Ok(ObjectDetail {
        id: target.id(&item),
        manifest: Some(trim(item)),
        text: None,
    })
}

command! {
    pub OBJECT_GET = ["k8s", "object", "get"], Read,
    "Show one object of any kind, a custom resource (CRD) too: manifest and status",
    keywords: ["crd", "custom", "resource", "kind", "any", "generic", "manifest", "spec", "yaml", "certificate", "raw", "read", "single"],
    example: "k8s object get certificate qa/dev/orders-tls --fields manifest",
    run: object_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{k8s, run};

    #[test]
    fn a_custom_resource_comes_trimmed_by_the_id_object_list_prints() {
        let outcome = k8s(&["k8s", "object", "get", "cert", "qa/dev/orders-tls"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let detail = outcome.json();
        assert_eq!(detail["id"], "qa/dev/orders-tls");
        let manifest = &detail["manifest"];
        assert_eq!(manifest["spec"]["secretName"], "orders-tls");
        assert_eq!(manifest["status"]["conditions"][0]["reason"], "Ready");
        assert!(
            manifest["metadata"].get("managedFields").is_none(),
            "{manifest}"
        );
        assert_eq!(
            manifest["metadata"]["annotations"],
            json!({"cert-manager.io/issue-temporary-certificate": "false"})
        );

        let outcome = run(&[
            "k8s",
            "object",
            "get",
            "certificate",
            "orders-tls",
            "--yaml",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["id"], "qa/dev/orders-tls");
        assert!(
            outcome.json()["text"]
                .as_str()
                .unwrap()
                .starts_with("apiVersion:")
        );
    }

    #[test]
    fn a_cluster_scoped_object_takes_cluster_slash_name_and_refuses_a_namespace() {
        let outcome = k8s(&[
            "k8s",
            "object",
            "get",
            "node",
            "qa/aks-np1-vmss000001",
            "--fields",
            "id",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!({"id": "qa/aks-np1-vmss000001"}));

        let outcome = k8s(&["k8s", "object", "get", "node", "qa/dev/aks-np1-vmss000001"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("cluster-scoped"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_missing_object_of_a_custom_kind_points_at_object_list() {
        let outcome = run(&["k8s", "object", "get", "certificates", "nope"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli k8s object list certificates.cert-manager.io"),
            "{}",
            outcome.stderr
        );
    }
}
