//! `k8s object list`: objects of any kind, `kubectl get KIND -o json`, as
//! rows: whether the kind is namespaced (from `k8s kind`) decides the scope
//! and the id.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kind::kind_named;
use crate::kubectl::{At, age, items, limited, non_empty};

#[derive(clap::Args)]
pub struct ObjectListArgs {
    /// The kind, as `k8s kind list` names it: certificates, cert, Certificate,
    /// certificates.cert-manager.io
    kind: String,
    /// Part of the object's name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    /// Only objects with this label: app=api, or a key alone (repeatable, all must hold)
    #[arg(long)]
    label: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ObjectRow {
    /// What `k8s object get KIND` takes: cluster/namespace/name, or
    /// cluster/name for a cluster-scoped kind.
    id: String,
    kind: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    age: Option<String>,
    /// From its Ready or Available condition, when it has one.
    ready: Option<bool>,
    /// Its phase, else that condition's reason.
    status: Option<String>,
}

fn object_list(ctx: &Ctx, args: ObjectListArgs) -> Result<Vec<ObjectRow>> {
    let probe = args.at.cluster_wide(ctx)?;
    let kind = kind_named(ctx, &probe, &args.kind)?;
    let target = if kind.namespaced {
        args.at.listing(ctx)?
    } else {
        probe
    };
    let qualified = kind.qualified();
    let selector = args.label.join(",");
    let mut argv = vec!["get", qualified.as_str(), "-o", "json"];
    if !selector.is_empty() {
        argv.extend(["-l", selector.as_str()]);
    }
    let listed = target.json(ctx, &argv)?;
    let rows = items(&listed)
        .filter(|item| {
            args.name.as_deref().is_none_or(|part| {
                item["metadata"]["name"]
                    .as_str()
                    .is_some_and(|name| name.contains(part))
            })
        })
        .map(|item| {
            let condition = readiness(item);
            ObjectRow {
                id: target.id(item),
                kind: item["kind"].as_str().unwrap_or(&kind.kind).to_owned(),
                namespace: target.row_namespace(item),
                age: age(&item["metadata"]["creationTimestamp"]),
                ready: condition.map(|condition| condition["status"] == "True"),
                status: non_empty(&item["status"]["phase"])
                    .or_else(|| condition.and_then(|condition| non_empty(&condition["reason"])))
                    .map(str::to_owned),
            }
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

/// The `Ready` or `Available` condition, which most kinds and the common
/// CRDs (cert-manager, Flux, KEDA, Gateway API's listeners aside) write.
fn readiness(item: &Value) -> Option<&Value> {
    item["status"]["conditions"]
        .as_array()?
        .iter()
        .find(|condition| matches!(condition["type"].as_str(), Some("Ready" | "Available")))
}

command! {
    pub OBJECT_LIST = ["k8s", "object", "list"], Read,
    "List objects of any kind, custom resources (CRDs) too: id, age, ready, status",
    keywords: ["crd", "crds", "custom", "resource", "resources", "kind", "generic", "certificates", "cert-manager", "gateway", "operator"],
    example: "k8s object list certificates --cluster qa --namespace dev --fields id,ready,status",
    run: object_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{k8s, run};

    #[test]
    fn custom_resources_list_with_their_readiness_and_ids_object_get_takes() {
        let outcome = run(&[
            "k8s",
            "object",
            "list",
            "cert",
            "--fields",
            "id,kind,ready,status",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "qa/dev/orders-tls", "kind": "Certificate", "ready": true, "status": "Ready"},
                {"id": "qa/dev/billing-tls", "kind": "Certificate", "ready": false, "status": "DoesNotExist"},
            ])
        );
        let outcome = run(&[
            "k8s",
            "object",
            "list",
            "certificates.cert-manager.io",
            "billing",
            "--fields",
            "id",
        ]);
        assert_eq!(outcome.json(), json!([{"id": "qa/dev/billing-tls"}]));
        let outcome = run(&[
            "k8s",
            "object",
            "list",
            "pods",
            "--label",
            "app=redis",
            "--fields",
            "id,status",
        ]);
        assert_eq!(
            outcome.json(),
            json!([{"id": "qa/dev/redis-0", "status": "Running"}])
        );
    }

    #[test]
    fn a_cluster_scoped_kind_lists_across_namespaces_with_cluster_name_ids() {
        let outcome = k8s(&[
            "k8s",
            "object",
            "list",
            "nodes",
            "--cluster",
            "qa",
            "--fields",
            "id,ready,status",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "qa/aks-np1-vmss000000", "ready": true, "status": "KubeletReady"},
                {"id": "qa/aks-np1-vmss000001", "ready": false, "status": "NodeStatusUnknown"},
            ])
        );
    }

    #[test]
    fn a_kind_the_cluster_does_not_serve_is_exit_4_pointing_at_kind_list() {
        let outcome = k8s(&["k8s", "object", "list", "certificates", "--cluster", "prod"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli k8s kind list --cluster prod"),
            "{}",
            outcome.stderr
        );
    }
}
