use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, items, non_empty, owned_by};
use crate::pod::{PodRow, ready};
use crate::shape::{EventRow, events_in};

use super::{DaemonSetRow, named, row};

#[derive(clap::Args)]
pub struct DaemonsetGetArgs {
    /// The daemonset: its id (cluster/namespace/name), namespace/name, or name
    daemonset: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DaemonSetDetail {
    #[serde(flatten)]
    row: DaemonSetRow,
    /// RollingUpdate or OnDelete.
    update_strategy: Option<String>,
    /// Nodes it should run on but does not, for a taint it does not tolerate
    /// or a selector: what `desired` leaves out.
    misscheduled: i64,
    /// Its pods that are not ready, with their node and status: kube-system's
    /// are outside most scopes, so they are named here rather than by a note.
    unready: Vec<PodRow>,
    /// The five newest events about it, newest first.
    events: Vec<EventRow>,
}

fn daemonset_get(ctx: &Ctx, args: DaemonsetGetArgs) -> Result<DaemonSetDetail> {
    let (target, name) = named(ctx, &args.at, &args.daemonset)?;
    let item = target.json(ctx, &["get", "daemonset", &name, "-o", "json"])?;
    let listed = target.json(ctx, &["get", "pods,events", "-o", "json"])?;
    let row = row(&target, &item)
        .ok_or_else(|| anyhow::anyhow!("kubectl answered a daemonset with no name"))?;
    Ok(DaemonSetDetail {
        row,
        update_strategy: non_empty(&item["spec"]["updateStrategy"]["type"]).map(str::to_owned),
        misscheduled: item["status"]["numberMisscheduled"].as_i64().unwrap_or(0),
        unready: items(&listed)
            .filter(|held| held["kind"] == "Pod" && owned_by(held, "DaemonSet", &name))
            .filter(|pod| !ready(pod))
            .filter_map(|pod| crate::pod::row(&target, pod))
            .collect(),
        events: events_in(&target, &listed, &item),
    })
}

command! {
    pub DAEMONSET_GET = ["k8s", "daemonset", "get"], Read,
    "Describe a daemonset, kube-system's too: unready pods, update strategy, events",
    keywords: ["ds", "agent", "cni", "csi", "unavailable"],
    example: "k8s daemonset get prod/kube-system/azure-cns",
    run: daemonset_get,
}

#[cfg(test)]
mod tests {
    use crate::testing::k8s;

    #[test]
    fn a_daemonset_short_of_a_node_names_its_unready_pod_and_where_it_runs() {
        let outcome = k8s(&["k8s", "daemonset", "get", "qa/kube-system/cilium"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["unavailable"], 1);
        assert_eq!(got["update_strategy"], "RollingUpdate");
        assert_eq!(got["unready"][0]["id"], "qa/kube-system/cilium-x7k2p");
        assert_eq!(got["unready"][0]["node"], "aks-np1-vmss000001");
        let outcome = k8s(&[
            "k8s",
            "daemonset",
            "get",
            "kube-proxy",
            "--cluster",
            "qa",
            "--namespace",
            "kube-system",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(outcome.json().get("unready").is_none(), "{outcome:?}");

        let outcome = k8s(&[
            "k8s",
            "daemonset",
            "get",
            "qa/kube-system/cilium",
            "--namespace",
            "dev",
        ]);
        assert_eq!(
            outcome.code, 2,
            "a ref and a flag that disagree: {outcome:?}"
        );
    }
}
