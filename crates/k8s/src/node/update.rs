use agent_cli_core::{Ctx, Effect, command};
use anyhow::Result;
use clap::ArgAction;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::At;

#[derive(clap::Args)]
pub struct NodeUpdateArgs {
    /// The node: its id (cluster/name) or name
    node: String,
    /// false cordons it (no new pod lands there; those on it stay), true uncordons it
    #[arg(long, action = ArgAction::Set)]
    schedulable: bool,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NodeUpdated {
    /// The node's id: what `node get` takes.
    id: String,
    schedulable: bool,
    /// kubectl's own line: `node/x cordoned`, or `already cordoned`.
    said: String,
}

fn node_update(ctx: &Ctx, args: NodeUpdateArgs) -> Result<NodeUpdated> {
    let (target, name) = args.at.named_cluster(ctx, &args.node)?;
    let verb = if args.schedulable {
        "uncordon"
    } else {
        "cordon"
    };
    let said = target.write(ctx, Effect::Destructive, &[verb, &name])?;
    Ok(NodeUpdated {
        id: format!("{}/{name}", target.scope),
        schedulable: args.schedulable,
        said: said.trim().to_owned(),
    })
}

command! {
    pub NODE_UPDATE = ["k8s", "node", "update"], Destructive,
    "Cordon or uncordon a node: whether new pods may be scheduled on it",
    keywords: ["cordon", "uncordon", "unschedulable", "maintenance", "mark"],
    example: "k8s node update prod/aks-user-27182818-vmss000001 --schedulable false --yes",
    run: node_update,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{dry_run, k8s};

    #[test]
    fn schedulable_false_cordons_and_true_uncordons_the_node_by_its_cluster_id() {
        let plans = dry_run(&[
            "k8s",
            "node",
            "update",
            "qa/aks-np1-vmss000001",
            "--schedulable",
            "false",
        ]);
        assert_eq!(
            plans[0]["run"],
            json!([
                "kubectl",
                "--context",
                "aks-qa",
                "--request-timeout=10s",
                "cordon",
                "aks-np1-vmss000001"
            ])
        );
        let outcome = k8s(&[
            "k8s",
            "node",
            "update",
            "qa/aks-np1-vmss000001",
            "--schedulable",
            "true",
            "--yes",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "qa/aks-np1-vmss000001", "schedulable": true,
                   "said": "node/aks-np1-vmss000001 uncordoned"})
        );
        let outcome = k8s(&[
            "k8s",
            "node",
            "update",
            "qa/nope",
            "--schedulable",
            "true",
            "--yes",
        ]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
