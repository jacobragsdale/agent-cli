use agent_cli_core::{Ctx, command};
use anyhow::Result;
use serde_json::Value;

use crate::kubectl::{At, items, limited, name_has};

use super::{NodeRow, row, top};

#[derive(clap::Args)]
pub struct NodeListArgs {
    /// Part of the node name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn node_list(ctx: &Ctx, args: NodeListArgs) -> Result<Vec<NodeRow>> {
    let target = args.at.cluster_wide(ctx)?;
    // Every pod of the cluster, whatever namespaces the scope names: a
    // node's requests are all of its pods'.
    // ponytail: one call for every node and pod; `--field-selector
    // spec.nodeName` per node if clusters grow to thousands of pods.
    let listed = target.json(
        ctx,
        &["get", "nodes,pods", "-o", "json", "--all-namespaces"],
    )?;
    let (nodes, pods): (Vec<&Value>, Vec<&Value>) =
        items(&listed).partition(|item| item["kind"] == "Node");
    let used = top(ctx, &target, &["nodes"]).unwrap_or_default();
    let rows = nodes
        .into_iter()
        .filter(|node| name_has(node, args.name.as_deref()))
        .map(|node| {
            let name = node["metadata"]["name"].as_str().unwrap_or_default();
            row(&target, node, &pods, used.get(name))
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub NODE_LIST = ["k8s", "node", "list"], Read,
    "List nodes: ready, cordoned, taints, CPU and memory requests and usage",
    keywords: ["notready", "cordoned", "unschedulable", "capacity", "pressure", "allocatable", "insufficient", "top", "utilization"],
    example: "k8s node list --cluster prod --fields id,ready,schedulable,pool,cpu",
    run: node_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn nodes_sum_their_pods_requests_against_allocatable_beside_what_top_measured() {
        let outcome = k8s(&["k8s", "node", "list", "--cluster", "qa"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"id": "qa/aks-np1-vmss000000", "name": "aks-np1-vmss000000", "ready": true,
                "schedulable": true, "pool": "np1", "nodepool": "aks-qa/np1", "zone": "westeurope-1", "version": "v1.30.4",
                "taints": ["node.kubernetes.io/disk-pressure:NoSchedule"],
                "cpu": {"requested": "250m", "allocatable": "1900m", "requested_percent": 13,
                    "usage": "320m", "usage_percent": 16},
                "memory": {"requested": "256Mi", "allocatable": "5726Mi", "requested_percent": 4,
                    "usage": "2210Mi", "usage_percent": 38},
                "pressure": ["DiskPressure"], "pod_count": 9, "age": rows[0]["age"]})
        );
        assert_eq!(rows[1]["ready"], false);
        assert!(rows[1]["cpu"].get("usage").is_none(), "{}", rows[1]);
        assert!(
            !outcome.stderr.contains("usage not read"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_cluster_without_metrics_server_lists_its_nodes_with_a_note() {
        let outcome = k8s(&["k8s", "node", "list", "--cluster", "prod"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!([]));
        assert!(
            outcome
                .stderr
                .contains("[usage not read: Metrics API not available]"),
            "{}",
            outcome.stderr
        );
    }
}
