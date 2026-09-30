use agent_cli_core::{Ctx, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;

use crate::client::{Airflow, At, Want, note_more};

use super::{TaskRow, task_row};

const TASK_STATES: [&str; 13] = [
    "none",
    "removed",
    "scheduled",
    "queued",
    "running",
    "success",
    "restarting",
    "failed",
    "up_for_retry",
    "up_for_reschedule",
    "upstream_failed",
    "skipped",
    "deferred",
];

#[derive(clap::Args)]
pub struct TaskListArgs {
    /// The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
    run: String,
    /// The DAG, when RUN is a bare run id
    #[arg(long)]
    dag: Option<String>,
    /// Only task instances in this state (repeatable)
    #[arg(long, value_parser = PossibleValuesParser::new(TASK_STATES))]
    state: Vec<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn task_list(ctx: &Ctx, args: TaskListArgs) -> Result<Vec<TaskRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = airflow.locate(
        ctx,
        args.at.instance.as_deref(),
        &args.run,
        Want::Run,
        args.dag.as_deref(),
        None,
    )?;
    let mut query = vec!["order_by=start_date".to_owned()];
    query.extend(args.state.iter().map(|state| format!("state={state}")));
    let (tasks, total) = client.list(
        &format!("{}/taskInstances", id.run_path()),
        &query.join("&"),
        "task_instances",
        args.limit,
    )?;
    note_more(ctx, tasks.len(), total);
    Ok(tasks.iter().map(task_row).collect())
}

command! {
    pub TASK_LIST = ["airflow", "task", "list"], Read,
    "List a DAG run's task instances with their state, try, times and host",
    keywords: ["instances", "states", "steps", "mapped", "running", "failed", "tasks", "hostname"],
    example: "airflow task list etl_nightly/latest --state failed --fields id,end,hostname",
    run: task_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{RUN, RUN_PATH, airflow, paths, ti, tis};

    #[test]
    fn task_list_prints_ids_with_their_try_and_the_host() {
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "task",
                "list",
                &format!("etl_nightly/{RUN}"),
                "--state",
                "failed",
                "--state",
                "upstream_failed",
            ],
            vec![tis(vec![
                ti("load_orders", "failed", 2),
                ti("publish_report", "upstream_failed", 0),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"id": format!("etl_nightly/{RUN}/load_orders/2"), "state": "failed", "try_number": 2,
                "max_tries": 1, "start": "2026-09-29T00:41:07Z", "end": "2026-09-29T00:43:55Z", "duration": 168,
                "operator": "PythonOperator", "hostname": "etl-nightly-load-orders-q8x1k2vz"})
        );
        assert_eq!(
            rows[1]["id"],
            format!("etl_nightly/{RUN}/publish_report"),
            "no try yet"
        );
        assert_eq!(
            paths(&transport),
            [format!(
                "{RUN_PATH}/taskInstances?order_by=start_date&state=failed&state=upstream_failed&limit=50&offset=0"
            )]
        );
    }
}
