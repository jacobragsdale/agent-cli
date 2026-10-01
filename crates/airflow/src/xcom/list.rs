//! `airflow xcom list`: a task instance's XCom keys
//! (`GET …/taskInstances/TASK/xcomEntries`), never their values.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Airflow, At, Want, note_more, stamp};

use super::{entries_path, map_query, task_id};

#[derive(clap::Args)]
pub struct XcomListArgs {
    /// The task instance: DAG/RUN/TASK[:MAP], or its Airflow UI URL
    task: String,
    /// The DAG, when the id leaves it out
    #[arg(long)]
    dag: Option<String>,
    /// The run id, when the id leaves it out
    #[arg(long)]
    run: Option<String>,
    #[command(flatten)]
    at: At,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct XcomRow {
    /// DAG/RUN/TASK[:MAP]@KEY: what xcom get takes.
    id: String,
    key: String,
    /// The map index, for a mapped task.
    map: Option<i64>,
    /// When the task pushed it.
    time: Option<String>,
}

fn xcom_list(ctx: &Ctx, args: XcomListArgs) -> Result<Vec<XcomRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = airflow.locate(
        ctx,
        args.at.instance.as_deref(),
        &args.task,
        Want::Task,
        args.dag.as_deref(),
        args.run.as_deref(),
    )?;
    let (entries, total) = client.list(
        &entries_path(&id),
        &map_query(&id),
        "xcom_entries",
        args.limit,
    )?;
    note_more(ctx, entries.len(), total);
    Ok(entries
        .iter()
        .map(|entry| {
            let key = entry["key"].as_str().unwrap_or_default().to_owned();
            let map = entry["map_index"].as_i64().filter(|map| *map >= 0);
            XcomRow {
                id: format!("{}@{key}", task_id(&id, map)),
                key,
                map,
                time: stamp(&entry["timestamp"]),
            }
        })
        .collect())
}

command! {
    pub XCOM_LIST = ["airflow", "xcom", "list"], Read,
    "List the XComs a task instance pushed: their keys, not their values",
    keywords: ["return", "value", "output", "result", "pushed", "keys", "upstream"],
    example: "airflow xcom list etl_nightly/latest/extract_orders --fields id,key",
    run: xcom_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RUN, RUN_PATH, airflow, paths};

    #[test]
    fn xcom_list_prints_each_keys_id_and_asks_a_mapped_tasks_index() {
        let entry = |key: &str, map: i64| {
            json!({"key": key, "timestamp": "2026-09-29T00:12:31.5+00:00", "map_index": map,
                "task_id": "extract_orders", "dag_id": "etl_nightly", "run_id": RUN})
        };
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "xcom",
                "list",
                &format!("etl_nightly/{RUN}/extract_orders"),
            ],
            vec![Answer::json(
                &json!({"xcom_entries": [entry("return_value", -1),
                entry("row_count", -1)], "total_entries": 3}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": format!("etl_nightly/{RUN}/extract_orders@return_value"), "key": "return_value",
                 "time": "2026-09-29T00:12:31Z"},
                {"id": format!("etl_nightly/{RUN}/extract_orders@row_count"), "key": "row_count",
                 "time": "2026-09-29T00:12:31Z"}
            ])
        );
        assert!(
            outcome.stderr.contains("[2 of 3; --limit N]"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            paths(&transport),
            [format!(
                "{RUN_PATH}/taskInstances/extract_orders/xcomEntries?limit=50&offset=0"
            )]
        );
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "xcom",
                "list",
                "extract_orders:3",
                "--dag",
                "etl_nightly",
                "--run",
                RUN,
            ],
            vec![Answer::json(
                &json!({"xcom_entries": [entry("return_value", 3)], "total_entries": 1}),
            )],
        );
        assert_eq!(
            outcome.json()[0]["id"],
            format!("etl_nightly/{RUN}/extract_orders:3@return_value")
        );
        assert_eq!(outcome.json()[0]["map"], 3);
        assert_eq!(
            paths(&transport),
            [format!(
                "{RUN_PATH}/taskInstances/extract_orders/xcomEntries?map_index=3&limit=50&offset=0"
            )]
        );
    }
}
