//! `aks nodepool get`: one agent pool and its `upgradeProfiles/default`,
//! which holds the newest node image AKS offers it.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::aks::nodepool::{Pool, next, pool, pool_name};
use crate::aks::{Place, locate};
use crate::client::text;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct NodepoolGetArgs {
    /// The pool: CLUSTER/POOL as `aks nodepool list` prints it, its ARM id, or its portal link
    pool: String,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PoolDetail {
    #[serde(flatten)]
    pool: Pool,
    /// Extra nodes an upgrade adds at once: a count or a percentage.
    max_surge: Option<String>,
    max_unavailable: Option<String>,
    /// How long an upgrade waits for a node's pods to leave.
    drain_timeout_minutes: Option<u64>,
    /// Kubernetes versions it can move to.
    upgrades: Vec<String>,
    /// The newest node image AKS offers it; newer than `node_image` means a
    /// node image upgrade is waiting.
    newest_node_image: Option<String>,
}

fn nodepool_get(ctx: &Ctx, args: NodepoolGetArgs) -> Result<PoolDetail> {
    let azure = Azure::load(ctx)?;
    let (located, pool_ref) = locate(ctx, &azure, &args.pool, &args.place)?;
    let name = pool_name(&located.name, pool_ref)?;
    let held = located.get(ctx, &format!("/agentPools/{name}"))?;
    let profile = located.get(ctx, &format!("/agentPools/{name}/upgradeProfiles/default"))?;
    let settings = &held["properties"]["upgradeSettings"];
    let detail = PoolDetail {
        pool: pool(&located.name, &held),
        max_surge: text(&settings["maxSurge"]),
        max_unavailable: text(&settings["maxUnavailable"]),
        drain_timeout_minutes: settings["drainTimeoutInMinutes"].as_u64(),
        upgrades: profile["properties"]["upgrades"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|upgrade| text(&upgrade["kubernetesVersion"]))
            .collect(),
        newest_node_image: text(&profile["properties"]["latestNodeImageVersion"]),
    };
    next(ctx, &detail.pool);
    Ok(detail)
}

command! {
    pub NODEPOOL_GET = ["aks", "nodepool", "get"], Read,
    "Show an AKS node pool: autoscaler, surge settings, newest node image available",
    keywords: ["agent", "autoscaler", "maximum", "surge", "image", "nodes", "vmss", "capacity"],
    example: "aks nodepool get aks-contoso-prod/user --fields count,autoscale,newest_node_image",
    run: nodepool_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AKS;
    use crate::testing::{self, AKS_ID, agent_pool, azure};

    fn profile() -> Answer {
        Answer::json(&json!({"name": "default", "properties": {
            "kubernetesVersion": "1.30.4", "osType": "Linux",
            "latestNodeImageVersion": "AKSUbuntu-2204gen2containerd-202510.03.0",
            "upgrades": [{"kubernetesVersion": "1.30.9"}, {"kubernetesVersion": "1.31.5", "isPreview": false}]}}))
    }

    #[test]
    fn get_adds_surge_and_the_newest_image_and_a_pool_at_its_maximum_says_what_waits() {
        let (outcome, transport) = azure(
            &[AKS],
            &["aks", "nodepool", "get", "aks-contoso-dev/user"],
            vec![
                testing::inventory(vec![testing::cluster("aks-contoso-dev")]),
                Answer::json(&agent_pool("user", 5, Some(5))),
                profile(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], "aks-contoso-dev/user");
        assert_eq!(got["max_surge"], "33%");
        assert_eq!(got["upgrades"], json!(["1.30.9", "1.31.5"]));
        assert_eq!(
            got["newest_node_image"],
            "AKSUbuntu-2204gen2containerd-202510.03.0"
        );
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli k8s event list --cluster aks-contoso-dev  (the autoscaler is at its maximum of 5 nodes"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent()[2].url,
            format!(
                "https://management.azure.com{AKS_ID}/agentPools/user/upgradeProfiles/default?api-version=2026-07-01"
            )
        );
    }

    #[test]
    fn a_pool_id_needs_no_inventory_and_a_bare_cluster_is_not_a_pool() {
        let id = format!("{AKS_ID}/agentPools/system");
        let (outcome, transport) = azure(
            &[AKS],
            &["aks", "nodepool", "get", &id],
            vec![Answer::json(&agent_pool("system", 3, None)), profile()],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(transport.sent().len(), 2);
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);

        let (outcome, _) = azure(&[AKS], &["aks", "nodepool", "get", AKS_ID], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli aks nodepool list aks-contoso-dev"),
            "{}",
            outcome.stderr
        );
    }
}
