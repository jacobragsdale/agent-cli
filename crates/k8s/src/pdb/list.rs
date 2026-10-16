use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, items, limited, name_has};
use crate::shape::selector_text;

#[derive(clap::Args)]
pub struct PdbListArgs {
    /// Part of the budget's name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PdbRow {
    /// `cluster/namespace/name`: what `k8s object get pdb` takes.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// The pods it guards: `app=orders-db`.
    selector: Option<String>,
    /// A count or a percentage, as the budget says it.
    min_available: Option<String>,
    max_unavailable: Option<String>,
    /// Pods that may be evicted now. 0 blocks every voluntary eviction: a
    /// node drain, so an AKS node pool upgrade, waits on it.
    allowed_disruptions: i64,
    /// Healthy pods now, healthy pods it wants, and pods it guards.
    current_healthy: i64,
    desired_healthy: i64,
    expected: i64,
}

fn pdb_list(ctx: &Ctx, args: PdbListArgs) -> Result<Vec<PdbRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "poddisruptionbudgets", "-o", "json"])?;
    // A count or a percentage: the API keeps either as it was written.
    let bound = |held: &Value| match held {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    };
    let rows = items(&listed)
        .filter(|item| name_has(item, args.name.as_deref()))
        .filter_map(|item| {
            let spec = &item["spec"];
            let status = &item["status"];
            let count = |key: &str| status[key].as_i64().unwrap_or(0);
            Some(PdbRow {
                id: target.id(item),
                name: item["metadata"]["name"].as_str()?.to_owned(),
                namespace: target.row_namespace(item),
                selector: selector_text(&spec["selector"]),
                min_available: bound(&spec["minAvailable"]),
                max_unavailable: bound(&spec["maxUnavailable"]),
                allowed_disruptions: count("disruptionsAllowed"),
                current_healthy: count("currentHealthy"),
                desired_healthy: count("desiredHealthy"),
                expected: count("expectedPods"),
            })
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub PDB_LIST = ["k8s", "pdb", "list"], Read,
    "List disruption budgets (PDBs): allowed disruptions; 0 blocks drains, upgrades",
    keywords: ["disruption", "budget", "drain", "evict", "eviction", "upgrade", "blocked", "minavailable"],
    example: "k8s pdb list --cluster prod --namespace web --fields id,selector,allowed_disruptions",
    run: pdb_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn pdb_list_shows_how_many_pods_may_be_evicted() {
        let outcome = run(&["k8s", "pdb", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "qa/dev/redis", "name": "redis", "selector": "app=redis",
                "min_available": "1", "allowed_disruptions": 0, "current_healthy": 1,
                "desired_healthy": 1, "expected": 1},
                {"id": "qa/dev/orders-api", "name": "orders-api", "selector": "app=orders-api",
                "max_unavailable": "25%", "allowed_disruptions": 1, "current_healthy": 2,
                "desired_healthy": 1, "expected": 2}])
        );
    }
}
