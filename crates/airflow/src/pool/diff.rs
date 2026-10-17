//! `airflow pool diff`: two instances' pools by name: those on one side
//! only, slots, description and whether deferred tasks take a slot. What
//! the pools hold now (running, queued) is the moment's, not their
//! configuration, and is left out.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, Rules, command};
use anyhow::Result;
use serde_json::json;

use crate::diff::Instances;

fn pool_diff(ctx: &Ctx, args: Instances) -> Result<Diff> {
    args.compare(ctx, &Rules::PLAIN, |client| {
        let (items, _) = client.list("pools", "", "pools", 1000)?;
        let rows: BTreeMap<_, _> = items
            .iter()
            .filter_map(|pool| {
                let name = pool["name"].as_str()?.to_owned();
                let row = json!({"slots": pool["slots"], "description": pool["description"],
                    "include_deferred": pool["include_deferred"]});
                Some((name, row))
            })
            .collect();
        Ok((rows.into_iter().collect(), BTreeMap::new()))
    })
}

command! {
    pub POOL_DIFF = ["airflow", "pool", "diff"], Read,
    "Compare two instances' pools: those missing on one side, slots, description",
    keywords: ["staging", "missing", "slots", "concurrency"],
    example: "airflow pool diff qa prod",
    run: pool_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{TWO, airflow_with};

    #[test]
    fn pools_differ_by_slots_and_what_one_side_lacks() {
        let qa = json!({"pools": [{"name": "default_pool", "slots": 32, "description": "Default pool",
            "include_deferred": false, "queued_slots": 4}], "total_entries": 1});
        let prod = json!({"pools": [
            {"name": "default_pool", "slots": 128, "description": "Default pool", "include_deferred": false, "queued_slots": 0},
            {"name": "warehouse_writes", "slots": 2, "description": "Writes", "include_deferred": false},
        ], "total_entries": 2});
        let (outcome, _) = airflow_with(
            TWO,
            &["airflow", "pool", "diff", "qa", "prod"],
            vec![Answer::json(&qa), Answer::json(&prod)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["changes"],
            json!([
                {"path": "default_pool.slots", "left": 32, "right": 128},
                {"path": "warehouse_writes", "right": {"slots": 2, "description": "Writes", "include_deferred": false}},
            ])
        );
    }
}
