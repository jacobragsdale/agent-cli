use agent_cli_core::{Ctx, Failure, When, command};
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
    /// Only runs due at or after this (their run_after; on Airflow 2 their logical date)
    #[arg(long)]
    since: Option<When>,
    /// Only runs due at or before this (on Airflow 2, their logical date)
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
    let v1 = client.v1()?;
    if v1 && !args.kind.is_empty() {
        return Err(Failure::usage(
            "Airflow 2's API cannot filter runs by type; each row's type says it",
        )
        .hint("drop --type")
        .into());
    }
    // Airflow 2 has no run_after: its runs filter on their logical date.
    let due = if v1 { "execution_date" } else { "run_after" };
    let mut query = vec![client.newest_first()?.to_owned()];
    query.extend(args.state.iter().map(|state| format!("state={state}")));
    query.extend(args.kind.iter().map(|kind| format!("run_type={kind}")));
    if let Some(since) = args.since {
        query.push(format!("{due}_gte={}", query_value(&since.utc())));
    }
    if let Some(until) = args.until {
        query.push(format!("{due}_lte={}", query_value(&until.utc())));
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

    #[test]
    fn run_list_on_airflow_2_filters_on_the_logical_date_and_refuses_a_type() {
        let (outcome, transport) = crate::testing::airflow_v1(
            &[
                "airflow",
                "run",
                "list",
                "--state",
                "failed",
                "--since",
                "2026-09-28T00:00:00Z",
            ],
            vec![Answer::json(
                &json!({"dag_runs": [crate::testing::run_v1("failed")], "total_entries": 1}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["run_after"], "2026-09-29T00:00:00Z");
        assert_eq!(outcome.json()[0]["logical_date"], "2026-09-28T00:00:00Z");
        assert_eq!(
            paths(&transport),
            [
                "dags/~/dagRuns?order_by=-execution_date&state=failed&execution_date_gte=2026-09-28T00%3A00%3A00Z&limit=50&offset=0"
            ]
        );
        let (outcome, transport) =
            crate::testing::airflow_v1(&["airflow", "run", "list", "--type", "manual"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }
}
