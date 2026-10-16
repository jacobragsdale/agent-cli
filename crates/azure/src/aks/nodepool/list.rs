//! `aks nodepool list`: ARM's agentPools of one cluster.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::aks::nodepool::{Pool, pool};
use crate::aks::{Place, locate};
use crate::client::limited;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct NodepoolListArgs {
    /// The cluster: its name as `aks cluster list` shows it, its ARM id, or its portal link
    cluster: String,
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn nodepool_list(ctx: &Ctx, args: NodepoolListArgs) -> Result<Vec<Pool>> {
    let azure = Azure::load(ctx)?;
    let (located, _) = locate(ctx, &azure, &args.cluster, &args.place)?;
    // ponytail: one page; AKS caps a cluster at 100 pools, which ARM sends at once.
    let answer = located.get(ctx, "/agentPools")?;
    let rows = answer["value"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|held| pool(&located.name, held))
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub NODEPOOL_LIST = ["aks", "nodepool", "list"], Read,
    "List an AKS cluster's node pools: VM size, node count, autoscaler bounds, subnet",
    keywords: ["agent", "pools", "nodes", "vmss", "autoscaler", "sku", "taints", "zones"],
    example: "aks nodepool list aks-contoso-prod --fields id,mode,count,autoscale,state",
    run: nodepool_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AKS;
    use crate::testing::{self, AKS_ID, agent_pool, azure};

    #[test]
    fn list_reads_every_pool_of_the_cluster() {
        let (outcome, transport) = azure(
            &[AKS],
            &["aks", "nodepool", "list", "aks-contoso-dev"],
            vec![
                testing::inventory(vec![testing::cluster("aks-contoso-dev")]),
                Answer::json(
                    &json!({"value": [agent_pool("system", 3, None), agent_pool("user", 5, Some(5))]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["id"], "aks-contoso-dev/system");
        assert_eq!(rows[0].get("autoscale"), None);
        assert_eq!(rows[1]["autoscale"], json!({"min": 1, "max": 5}));
        assert_eq!(
            rows[1]["subnet"].as_str().unwrap().rsplit('/').next(),
            Some("aks")
        );
        assert_eq!(rows[1]["taints"], json!(["sku=gpu:NoSchedule"]));
        assert!(
            !outcome.stderr.contains("[next:"),
            "lists print no next: {}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent()[1].url,
            format!("https://management.azure.com{AKS_ID}/agentPools?api-version=2026-07-01")
        );
    }
}
