use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command, redact_value};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Airflow, text};

use super::{RunIdArgs, failed_ids, run_row};

/// `run get` counts at most this many task instances.
const MOST_TASKS: usize = 1000;

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunDetail {
    id: String,
    state: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    run_after: Option<String>,
    logical_date: Option<String>,
    start: Option<String>,
    end: Option<String>,
    duration: Option<i64>,
    triggered_by: Option<String>,
    /// Who triggered it (Airflow 3.1+).
    user: Option<String>,
    conf: Value,
    note: Option<String>,
    dag_version: Option<i64>,
    /// Task instances by state.
    tasks: BTreeMap<String, usize>,
    /// The failed task instances (root causes; upstream_failed only counts):
    /// ids task logs takes.
    failed: Vec<String>,
}

fn run_get(ctx: &Ctx, args: RunIdArgs) -> Result<RunDetail> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = args.locate(&airflow, ctx)?;
    let run = client.get(&id.run_path())?;
    let (tasks, total) = client.list(
        &format!("{}/taskInstances", id.run_path()),
        "",
        "task_instances",
        MOST_TASKS,
    )?;
    if total.is_some_and(|total| total > tasks.len()) {
        ctx.note(format!(
            "[counted the first {} of {} task instances]",
            tasks.len(),
            total.unwrap_or_default()
        ));
    }
    let mut counts = BTreeMap::new();
    for task in &tasks {
        let state = task["state"].as_str().unwrap_or("none").to_owned();
        *counts.entry(state).or_default() += 1;
    }
    let row = run_row(&run);
    Ok(RunDetail {
        id: row.id,
        state: row.state,
        kind: row.kind,
        run_after: row.run_after,
        logical_date: row.logical_date,
        start: row.start,
        end: row.end,
        duration: row.duration,
        triggered_by: row.triggered_by,
        user: text(&run["triggering_user_name"]),
        // Airflow masks secrets in logs and rendered fields, but not here.
        conf: redact_value(run["conf"].clone()),
        note: text(&run["note"]),
        dag_version: run["dag_versions"]
            .as_array()
            .and_then(|versions| versions.last())
            .and_then(|version| version["version_number"].as_i64()),
        tasks: counts,
        failed: failed_ids(&tasks),
    })
}

command! {
    pub RUN_GET = ["airflow", "run", "get"], Read,
    "Show a DAG run's state, task counts by state, and the tasks that failed",
    keywords: ["status", "state", "progress", "summary", "why", "failed", "latest"],
    example: "airflow run get etl_nightly/latest --fields id,state,tasks,failed",
    run: run_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RUN, RUN_PATH, airflow, paths, run, tasks, ti};

    #[test]
    fn run_get_counts_tasks_by_state_and_names_the_failed_ones_with_their_try() {
        let (outcome, transport) = airflow(
            &["airflow", "run", "get", "etl_nightly/latest"],
            vec![
                Answer::json(&json!({"dag_runs": [run("failed")], "total_entries": 1})),
                Answer::json(&run("failed")),
                tasks(vec![
                    ti("extract_orders", "success", 1),
                    ti("load_orders", "failed", 2),
                    ti("publish_report", "upstream_failed", 0),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], format!("etl_nightly/{RUN}"));
        assert_eq!(
            got["tasks"],
            json!({"failed": 1, "success": 1, "upstream_failed": 1})
        );
        assert_eq!(
            got["failed"],
            json!([format!("etl_nightly/{RUN}/load_orders/2")])
        );
        assert_eq!(got["dag_version"], 7);
        assert_eq!(got["conf"]["day"], "2026-09-28");
        assert_ne!(
            got["conf"]["api_token"], "s3cr3t-conf-value",
            "conf is redacted"
        );
        assert_eq!(
            paths(&transport),
            [
                "dags/etl_nightly/dagRuns?order_by=-run_after&limit=1".to_owned(),
                RUN_PATH.to_owned(),
                format!("{RUN_PATH}/taskInstances?limit=100&offset=0"),
            ]
        );
    }
}
