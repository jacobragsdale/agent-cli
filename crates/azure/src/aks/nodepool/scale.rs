//! `aks nodepool scale`: sets a manual pool's node count. ARM has no PATCH
//! for agentPools, so this is a PUT of the pool as read with only `count`
//! changed: a PUT of `count` alone would reset every other setting.

use agent_cli_core::{Ctx, Effect, Failure, Method, Request, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::aks::nodepool::pool_name;
use crate::aks::{Place, locate};
use crate::client::{ARM, bearer, text};
use crate::config::Azure;

/// What ARM fills in and refuses or ignores on a PUT.
// VERIFY(work): that a PUT of the pool's own GET, less these, is taken as is.
const READ_ONLY: &[&str] = &[
    "provisioningState",
    "currentOrchestratorVersion",
    "nodeImageVersion",
    "eTag",
    "status",
];

#[derive(clap::Args)]
pub struct NodepoolScaleArgs {
    /// The pool: CLUSTER/POOL as `aks nodepool list` prints it, its ARM id, or its portal link
    pool: String,
    /// How many nodes it should have
    #[arg(long)]
    nodes: u64,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Scaled {
    /// `cluster/pool`.
    id: String,
    previous: Option<u64>,
    count: u64,
    /// What ARM says it is doing: Scaling, then Succeeded.
    state: Option<String>,
}

fn nodepool_scale(ctx: &Ctx, args: NodepoolScaleArgs) -> Result<Scaled> {
    let azure = Azure::load(ctx)?;
    let (located, pool_ref) = locate(ctx, &azure, &args.pool, &args.place)?;
    let name = pool_name(&located.name, pool_ref)?;
    let below = format!("/agentPools/{name}");
    let held = located.get(ctx, &below)?;
    let id = format!("{}/{name}", located.name);
    let mut props = held["properties"].clone();
    if props["enableAutoScaling"].as_bool() == Some(true) {
        return Err(Failure::usage(format!(
            "{id}'s node count belongs to its autoscaler ({} to {} nodes); a count set by hand would be undone",
            props["minCount"], props["maxCount"]
        ))
        .hint(format!(
            "change its bounds instead: az aks nodepool update --resource-group {} --cluster-name {} --name {name} --update-cluster-autoscaler --min-count MIN --max-count MAX",
            located.resource_group, located.name
        ))
        .into());
    }
    let previous = props["count"].as_u64();
    if let Some(fields) = props.as_object_mut() {
        fields.retain(|key, _| !READ_ONLY.contains(&key.as_str()));
        fields.insert("count".to_owned(), json!(args.nodes));
    }
    let mint = bearer(ctx, ARM);
    let request = Request::new(Method::Put, located.url(&below))
        .json(json!({ "properties": props }))
        .auth(&mint);
    let answer = ctx.write(Effect::Destructive, request)?.json()?;
    Ok(Scaled {
        id,
        previous,
        count: args.nodes,
        state: text(&answer["properties"]["provisioningState"]),
    })
}

command! {
    pub NODEPOOL_SCALE = ["aks", "nodepool", "scale"], Destructive,
    "Set the node count of an AKS node pool whose autoscaler is off",
    keywords: ["agent", "nodes", "resize", "grow", "shrink", "capacity", "vmss"],
    example: "aks nodepool scale aks-contoso-prod/system --nodes 4 --yes",
    run: nodepool_scale,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, assert_dry_run};
    use serde_json::json;

    use crate::AKS;
    use crate::testing::{AKS_ID, agent_pool, azure};

    #[test]
    fn scale_puts_the_pool_back_with_only_its_count_changed() {
        let id = format!("{AKS_ID}/agentPools/system");
        let argv = ["aks", "nodepool", "scale", &id, "--nodes", "4"];
        let plans = assert_dry_run(
            &[AKS],
            &argv,
            vec![Answer::json(&agent_pool("system", 3, None))],
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(
            plans[0]["url"],
            format!("https://management.azure.com{id}?api-version=2026-07-01")
        );
        let sent = &plans[0]["body"]["properties"];
        assert_eq!(sent["count"], 4);
        assert_eq!(sent["vmSize"], "Standard_D4s_v5");
        assert_eq!(sent["nodeTaints"], json!(["sku=gpu:NoSchedule"]));
        assert_eq!(sent.get("provisioningState"), None);
        assert_eq!(sent.get("eTag"), None);

        let mut scaling = agent_pool("system", 4, None);
        scaling["properties"]["provisioningState"] = json!("Scaling");
        let (outcome, _) = azure(
            &[AKS],
            &[&argv[..], &["--yes"]].concat(),
            vec![
                Answer::json(&agent_pool("system", 3, None)),
                Answer::json(&scaling),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "aks-contoso-dev/system", "previous": 3, "count": 4, "state": "Scaling"})
        );
    }

    #[test]
    fn an_autoscaled_pool_is_refused_with_the_bounds_to_change() {
        let (outcome, transport) = azure(
            &[AKS],
            &[
                "aks",
                "nodepool",
                "scale",
                &format!("{AKS_ID}/agentPools/user"),
                "--nodes",
                "8",
                "--yes",
            ],
            vec![Answer::json(&agent_pool("user", 5, Some(5)))],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("--max-count MAX"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1, "only the read went out");
    }
}
