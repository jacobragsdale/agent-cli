//! A node pool as `list` and `get` print it, and the note a pool its
//! autoscaler can no longer grow earns.

pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod scale;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::text;

/// One agent pool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Pool {
    /// `cluster/pool`, what `aks nodepool get` and `scale` take.
    id: String,
    /// System or User.
    mode: Option<String>,
    vm_size: Option<String>,
    /// Nodes it has now.
    count: Option<u64>,
    /// The autoscaler's bounds; absent when it is off.
    autoscale: Option<Bounds>,
    version: Option<String>,
    node_image: Option<String>,
    os_sku: Option<String>,
    zones: Vec<String>,
    /// The subnet's ARM id; absent when AKS made the network.
    subnet: Option<String>,
    max_pods: Option<u64>,
    taints: Vec<String>,
    labels: Option<Value>,
    /// Succeeded, or what ARM is doing or failed at.
    state: Option<String>,
    /// Running or Stopped.
    power_state: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Bounds {
    min: Option<u64>,
    max: Option<u64>,
}

impl Pool {
    /// How many nodes it holds and its autoscaler's maximum, when it sits there.
    fn at_max(&self) -> Option<u64> {
        let max = self.autoscale.as_ref()?.max?;
        (self.count? >= max).then_some(max)
    }
}

/// `pool` (an agentPools item, or the pool itself) of `cluster`.
pub(crate) fn pool(cluster: &str, pool: &Value) -> Pool {
    let props = &pool["properties"];
    let list = |key: &str| -> Vec<String> {
        props[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(text)
            .collect()
    };
    let autoscaled = props["enableAutoScaling"].as_bool() == Some(true);
    Pool {
        id: format!("{cluster}/{}", text(&pool["name"]).unwrap_or_default()),
        mode: text(&props["mode"]),
        vm_size: text(&props["vmSize"]),
        count: props["count"].as_u64(),
        autoscale: autoscaled.then(|| Bounds {
            min: props["minCount"].as_u64(),
            max: props["maxCount"].as_u64(),
        }),
        version: text(&props["currentOrchestratorVersion"])
            .or_else(|| text(&props["orchestratorVersion"])),
        node_image: text(&props["nodeImageVersion"]),
        os_sku: text(&props["osSKU"]),
        zones: list("availabilityZones"),
        subnet: text(&props["vnetSubnetID"]),
        max_pods: props["maxPods"].as_u64(),
        taints: list("nodeTaints"),
        labels: Some(props["nodeLabels"].clone()).filter(|held| held.is_object()),
        state: text(&props["provisioningState"]),
        power_state: text(&props["powerState"]["code"]),
    }
}

/// The one note a pool that needs attention earns: one at its autoscaler's
/// maximum cannot take another pending pod, which is the next thing to see;
/// a failed one shows its cluster.
fn next(ctx: &Ctx, pool: &Pool) {
    let cluster = pool.id.split('/').next().unwrap_or_default();
    if let Some(max) = pool.at_max() {
        ctx.note(format!(
            "[next: agent-cli k8s event list --cluster {cluster}  (the autoscaler is at its maximum of {max} nodes, so pods that need more wait Pending; az aks nodepool update --update-cluster-autoscaler --max-count raises it)]"
        ));
    } else if pool
        .state
        .as_deref()
        .is_some_and(|state| state.eq_ignore_ascii_case("Failed"))
    {
        ctx.note(format!(
            "[next: agent-cli aks cluster get {cluster}  (the pool's last operation failed)]"
        ));
    }
}

/// The pool a `cluster/pool` ref or a pool's id named, or exit 2 when it
/// named only `cluster`.
pub(crate) fn pool_name(cluster: &str, pool: Option<String>) -> Result<String> {
    pool.filter(|pool| !pool.is_empty()).ok_or_else(|| {
        Failure::usage(format!("{cluster} is a cluster, not one of its node pools"))
            .hint(format!("its pools: agent-cli aks nodepool list {cluster}"))
            .into()
    })
}
