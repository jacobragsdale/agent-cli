use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;

use crate::client::{Airflow, At, note_more, query_value};
use crate::dag_run::RUN_STATES;

use super::{RunRow, run_row};

const RUN_TYPES: [&str; 4] = ["scheduled", "manual", "backfill", "asset_triggered"];

#[derive(clap::Args)]
pub struct RunListArgs {
    /// Only this DAG's runs (default: every DAG)
    #[arg(long)]
    dag: Option<String>,
    /// Only runs in this state (repeatable)
    #[arg(long, value_parser = PossibleValuesParser::new(RUN_STATES))]
    state: Vec<String>,
    /// Only runs of this type (repeatable)
    #[arg(long = "type", value_parser = PossibleValuesParser::new(RUN_TYPES))]
    kind: Vec<String>,
    /// Only runs due at or after this (their run_after)
    #[arg(long)]
    since: Option<When>,
    /// Only runs due at or before this
    #[arg(long)]
    until: Option<When>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn run_list(ctx: &Ctx, args: RunListArgs) -> Result<Vec<RunRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    // `~` is every DAG.
    let dag = args
        .dag
        .as_deref()
        .map_or_else(|| "~".to_owned(), crate::client::segment);
    let mut query = vec!["order_by=-run_after".to_owned()];
    query.extend(args.state.iter().map(|state| format!("state={state}")));
    query.extend(args.kind.iter().map(|kind| format!("run_type={kind}")));
    if let Some(since) = args.since {
        query.push(format!("run_after_gte={}", query_value(&since.utc())));
    }
    if let Some(until) = args.until {
        query.push(format!("run_after_lte={}", query_value(&until.utc())));
    }
    let (runs, total) = client.list(
        &format!("dags/{dag}/dagRuns"),
        &query.join("&"),
        "dag_runs",
        args.limit,
    )?;
    note_more(ctx, runs.len(), total);
    Ok(runs.iter().map(run_row).collect())
}

command! {
    pub RUN_LIST = ["airflow", "run", "list"], Read,
    "List DAG runs, newest first, across DAGs or for one",
    keywords: ["dagruns", "history", "recent", "failed", "executions", "last", "night", "nightly"],
    example: "airflow run list --dag etl_nightly --state failed --since 1d --fields id,state,end",
    run: run_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RUN, airflow, paths, run};

    #[test]
    fn run_list_spans_every_dag_by_default_and_filters_on_run_after() {
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "run",
                "list",
                "--state",
                "failed",
                "--type",
                "scheduled",
                "--since",
                "2026-09-28T12:00:00Z",
                "--until",
                "2026-09-29",
                "--fields",
                "id,state,duration",
            ],
            vec![Answer::json(
                &json!({"dag_runs": [run("failed")], "total_entries": 1}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": format!("etl_nightly/{RUN}"), "state": "failed", "duration": 2633}])
        );
        assert_eq!(
            paths(&transport),
            [
                "dags/~/dagRuns?order_by=-run_after&state=failed&run_type=scheduled&run_after_gte=2026-09-28T12%3A00%3A00Z&run_after_lte=2026-09-29T00%3A00%3A00Z&limit=50&offset=0"
            ]
        );
    }
}
