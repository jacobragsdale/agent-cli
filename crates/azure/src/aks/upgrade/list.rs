//! `aks upgrade list`: what a cluster can upgrade to. The cluster's
//! `upgradeProfiles/default` holds the Kubernetes versions of the control
//! plane and each pool; only each pool's own profile holds its newest node
//! image, so that is one more read per pool.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::aks::{Place, locate};
use crate::client::{limited, parallel, text};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct UpgradeListArgs {
    /// The cluster: its name as `aks cluster list` shows it, its ARM id, or its portal link
    cluster: String,
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// The control plane, or one pool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct UpgradeRow {
    /// `control-plane`, or the pool's `cluster/pool` id, which `aks nodepool get` takes.
    target: String,
    version: Option<String>,
    /// Versions it can move to; `(preview)` marks a preview.
    upgrades: Vec<String>,
    node_image: Option<String>,
    /// Set when a newer node image than `node_image` is waiting.
    newest_node_image: Option<String>,
}

fn versions(profile: &Value) -> Vec<String> {
    profile["upgrades"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|upgrade| {
            let version = text(&upgrade["kubernetesVersion"])?;
            Some(if upgrade["isPreview"].as_bool() == Some(true) {
                format!("{version} (preview)")
            } else {
                version
            })
        })
        .collect()
}

fn upgrade_list(ctx: &Ctx, args: UpgradeListArgs) -> Result<Vec<UpgradeRow>> {
    let azure = Azure::load(ctx)?;
    let (located, _) = locate(ctx, &azure, &args.cluster, &args.place)?;
    let profile = located.get(ctx, "/upgradeProfiles/default")?;
    let pools = located.get(ctx, "/agentPools")?;
    let pools: Vec<&Value> = pools["value"].as_array().into_iter().flatten().collect();
    let images = parallel(&pools, azure.parallel(), |held| {
        let name = text(&held["name"]).unwrap_or_default();
        located.get(ctx, &format!("/agentPools/{name}/upgradeProfiles/default"))
    });
    let control = &profile["properties"]["controlPlaneProfile"];
    let mut rows = vec![UpgradeRow {
        target: "control-plane".to_owned(),
        version: text(&control["kubernetesVersion"]),
        upgrades: versions(control),
        node_image: None,
        newest_node_image: None,
    }];
    for (held, image) in pools.into_iter().zip(images) {
        let name = text(&held["name"]).unwrap_or_default();
        let current = text(&held["properties"]["nodeImageVersion"]);
        let newest = text(&image?["properties"]["latestNodeImageVersion"]);
        let pool_profile = profile["properties"]["agentPoolProfiles"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|held| held["name"].as_str() == Some(&name))
            .unwrap_or(&Value::Null);
        rows.push(UpgradeRow {
            target: format!("{}/{name}", located.name),
            version: text(&held["properties"]["currentOrchestratorVersion"])
                .or_else(|| text(&pool_profile["kubernetesVersion"])),
            upgrades: versions(pool_profile),
            newest_node_image: newest.filter(|newest| Some(newest) != current.as_ref()),
            node_image: current,
        });
    }
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub UPGRADE_LIST = ["aks", "upgrade", "list"], Read,
    "List the Kubernetes versions and node images an AKS cluster can upgrade to",
    keywords: ["kubernetes", "version", "versions", "available", "control", "plane", "node", "image", "outdated"],
    example: "aks upgrade list aks-contoso-prod --fields target,version,upgrades,newest_node_image",
    run: upgrade_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AKS;
    use crate::testing::{AKS_ID, agent_pool, azure};

    #[test]
    fn upgrades_list_the_control_plane_then_each_pool_with_its_newest_image() {
        let mut system = agent_pool("system", 3, None);
        system["properties"]["nodeImageVersion"] =
            json!("AKSUbuntu-2204gen2containerd-202510.03.0");
        let (outcome, transport) = azure(
            &[AKS],
            &["aks", "upgrade", "list", AKS_ID],
            vec![
                Answer::json(&json!({"properties": {
                    "controlPlaneProfile": {"name": "master", "kubernetesVersion": "1.30.4",
                        "upgrades": [{"kubernetesVersion": "1.30.9"}, {"kubernetesVersion": "1.32.0", "isPreview": true}]},
                    "agentPoolProfiles": [
                        {"name": "system", "kubernetesVersion": "1.30.4", "upgrades": [{"kubernetesVersion": "1.30.9"}]},
                        {"name": "user", "kubernetesVersion": "1.30.4", "upgrades": [{"kubernetesVersion": "1.30.9"}]}]}})),
                Answer::json(&json!({"value": [system, agent_pool("user", 5, Some(5))]})),
                Answer::json(
                    &json!({"properties": {"latestNodeImageVersion": "AKSUbuntu-2204gen2containerd-202510.03.0"}}),
                ),
                Answer::json(
                    &json!({"properties": {"latestNodeImageVersion": "AKSUbuntu-2204gen2containerd-202510.03.0"}}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"target": "control-plane", "version": "1.30.4", "upgrades": ["1.30.9", "1.32.0 (preview)"]})
        );
        assert_eq!(rows[1]["target"], "aks-contoso-dev/system");
        assert_eq!(
            rows[1].get("newest_node_image"),
            None,
            "already on the newest"
        );
        assert_eq!(
            rows[2]["newest_node_image"],
            "AKSUbuntu-2204gen2containerd-202510.03.0"
        );
        assert_eq!(
            transport.sent()[0].url,
            format!(
                "https://management.azure.com{AKS_ID}/upgradeProfiles/default?api-version=2026-07-01"
            )
        );
    }
}
