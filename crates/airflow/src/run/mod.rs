//! DAG runs: history, a run's status with its task summary, triggering,
//! a bounded wait, and re-running what failed.

pub(crate) mod create;
pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod retry;
pub(crate) mod wait;

use agent_cli_core::Ctx;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Airflow, At, Client, Ref, Want, seconds, stamp, text, ti_id};

/// The positional every run command takes, and its leading piece as a flag.
#[derive(clap::Args)]
pub struct RunIdArgs {
    /// The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
    run: String,
    /// The DAG, when RUN is a bare run id
    #[arg(long)]
    dag: Option<String>,
    #[command(flatten)]
    at: At,
}

impl RunIdArgs {
    fn locate<'a>(&self, airflow: &'a Airflow, ctx: &'a Ctx) -> Result<(Client<'a>, Ref)> {
        airflow.locate(
            ctx,
            self.at.instance.as_deref(),
            &self.run,
            Want::Run,
            self.dag.as_deref(),
            None,
        )
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunRow {
    /// DAG/RUN: what run get, wait, retry and task list take.
    id: String,
    state: Option<String>,
    /// scheduled, manual, backfill or asset_triggered.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// When it was due.
    run_after: Option<String>,
    logical_date: Option<String>,
    start: Option<String>,
    end: Option<String>,
    /// Seconds.
    duration: Option<i64>,
    triggered_by: Option<String>,
}

fn run_row(run: &Value) -> RunRow {
    RunRow {
        id: format!(
            "{}/{}",
            run["dag_id"].as_str().unwrap_or_default(),
            run["dag_run_id"].as_str().unwrap_or_default()
        ),
        state: text(&run["state"]),
        kind: text(&run["run_type"]),
        run_after: stamp(&run["run_after"]),
        logical_date: stamp(&run["logical_date"]),
        start: stamp(&run["start_date"]),
        end: stamp(&run["end_date"]),
        duration: seconds(&run["start_date"], &run["end_date"]),
        triggered_by: text(&run["triggered_by"]),
    }
}

/// The failed task instances, with their try, so they paste into task logs.
fn failed_ids(tasks: &[Value]) -> Vec<String> {
    tasks
        .iter()
        .filter(|task| task["state"] == "failed")
        .map(|task| ti_id(task, true))
        .collect()
}

/// The task instances a clear's dry run listed, without a try: the next try
/// is the one that will run.
pub(crate) fn cleared_ids(preview: &Value) -> Vec<String> {
    preview["task_instances"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|task| ti_id(task, false))
        .collect()
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;

    use crate::testing::{RUN, RUN_PATH, airflow, paths, run, tasks};

    #[test]
    fn a_bare_run_id_takes_the_dag_from_the_flag_and_a_disagreeing_flag_is_exit_2() {
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "run",
                "get",
                RUN,
                "--dag",
                "etl_nightly",
                "--fields",
                "id",
            ],
            vec![Answer::json(&run("success")), tasks(vec![])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(paths(&transport)[0], RUN_PATH);
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "run",
                "get",
                &format!("etl_nightly/{RUN}"),
                "--dag",
                "orders_export",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("--dag says orders_export"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().is_empty());
        let url = "https://airflow.contoso.example/dags/etl_nightly/runs/scheduled__2026-09-29T00%3A00%3A00%2B00%3A00/details";
        let (outcome, transport) = airflow(
            &["airflow", "run", "get", url, "--fields", "id"],
            vec![Answer::json(&run("success")), tasks(vec![])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(paths(&transport)[0], RUN_PATH, "a UI URL names the run");
    }
}
