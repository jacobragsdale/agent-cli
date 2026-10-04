//! `airflow pool list`: every pool's slots and what holds them (`GET pools`).

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Airflow, At, note_more, text};

#[derive(clap::Args)]
pub struct PoolListArgs {
    #[command(flatten)]
    at: At,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PoolRow {
    /// The pool's name, as task get prints its pool.
    id: String,
    /// Its size; -1 is unlimited.
    slots: Option<i64>,
    /// Free now: a task queued for the pool waits while this is 0.
    open: Option<i64>,
    running: Option<i64>,
    queued: Option<i64>,
    scheduled: Option<i64>,
    deferred: Option<i64>,
    description: Option<String>,
}

fn pool_list(ctx: &Ctx, args: PoolListArgs) -> Result<Vec<PoolRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    let (pools, total) = client.list("pools", "", "pools", args.limit)?;
    note_more(ctx, pools.len(), total);
    Ok(pools
        .iter()
        .map(|pool| PoolRow {
            id: pool["name"].as_str().unwrap_or_default().to_owned(),
            slots: pool["slots"].as_i64(),
            open: pool["open_slots"].as_i64(),
            running: pool["running_slots"].as_i64(),
            queued: pool["queued_slots"].as_i64(),
            scheduled: pool["scheduled_slots"].as_i64(),
            deferred: pool["deferred_slots"].as_i64(),
            description: text(&pool["description"]),
        })
        .collect())
}

command! {
    pub POOL_LIST = ["airflow", "pool", "list"], Read,
    "List pools with their slots: open, queued, scheduled and deferred",
    keywords: ["queued", "stuck", "waiting", "capacity", "concurrency", "full", "free", "slot"],
    example: "airflow pool list --fields id,slots,open,queued",
    run: pool_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{airflow, paths};

    #[test]
    fn pool_list_prints_each_pools_slots() {
        let (outcome, transport) = airflow(
            &["airflow", "pool", "list"],
            vec![Answer::json(
                &json!({"pools": [{"name": "default_pool", "slots": 128,
                "description": "Default pool", "include_deferred": false, "occupied_slots": 3,
                "running_slots": 1, "queued_slots": 2, "scheduled_slots": 0, "open_slots": 125,
                "deferred_slots": 0}], "total_entries": 1}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "default_pool", "slots": 128, "open": 125, "running": 1, "queued": 2,
                "scheduled": 0, "deferred": 0, "description": "Default pool"}])
        );
        assert_eq!(paths(&transport), ["pools?limit=50&offset=0"]);
    }

    #[test]
    fn pool_list_reads_the_same_path_on_airflow_2() {
        let pool = json!({"name": "default_pool", "slots": 128, "open_slots": 127,
            "running_slots": 1, "queued_slots": 0, "scheduled_slots": 0, "deferred_slots": 0,
            "description": "Default pool", "occupied_slots": 1});
        let (outcome, transport) = crate::testing::airflow_v1(
            &["airflow", "pool", "list"],
            vec![Answer::json(&json!({"pools": [pool], "total_entries": 1}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["open"], 127);
        assert_eq!(
            transport.sent()[0].url,
            "https://airflow.contoso.example/api/v1/pools?limit=50&offset=0"
        );
    }
}
