use agent_cli_core::{Ctx, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;

use crate::client::{Airflow, At, note_more, query_value};

use super::{DagRow, dag_row};
use crate::dag_run::RUN_STATES;

#[derive(clap::Args)]
pub struct DagListArgs {
    /// Only DAG ids containing this
    pattern: Option<String>,
    /// Only DAGs with this tag (repeatable; any of them)
    #[arg(long)]
    tag: Vec<String>,
    /// true for paused DAGs only, false for active ones only
    #[arg(long)]
    paused: Option<bool>,
    /// Only DAGs whose last run ended in this state
    #[arg(long, value_parser = PossibleValuesParser::new(RUN_STATES))]
    last_state: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn dag_list(ctx: &Ctx, args: DagListArgs) -> Result<Vec<DagRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    let mut query = vec!["order_by=dag_id".to_owned()];
    if let Some(pattern) = &args.pattern {
        query.push(format!("dag_id_pattern={}", query_value(pattern.trim())));
    }
    for tag in &args.tag {
        query.push(format!("tags={}", query_value(tag)));
    }
    if let Some(paused) = args.paused {
        query.push(format!("paused={paused}"));
    }
    if let Some(state) = &args.last_state {
        query.push(format!("last_dag_run_state={state}"));
    }
    let (dags, total) = client.list("dags", &query.join("&"), "dags", args.limit)?;
    note_more(ctx, dags.len(), total);
    Ok(dags.iter().map(dag_row).collect())
}

command! {
    pub DAG_LIST = ["airflow", "dag", "list"], Read,
    "List DAGs with their schedule, next run and whether they are paused",
    keywords: ["dags", "workflows", "paused", "tags", "schedule", "owner", "failing", "all"],
    example: "airflow dag list --last-state failed --fields id,schedule,next_run",
    run: dag_list,
}

#[cfg(test)]
mod tests {

    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testing::{airflow, dag, paths};

    #[test]
    fn dag_list_filters_server_side_and_notes_the_rest() {
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "dag",
                "list",
                "etl",
                "--tag",
                "etl",
                "--tag",
                "orders team",
                "--paused",
                "false",
                "--last-state",
                "failed",
                "--limit",
                "1",
            ],
            vec![Answer::json(
                &json!({"dags": [dag("etl_nightly", false)], "total_entries": 3}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "etl_nightly", "paused": false, "schedule": "0 0 * * *",
                "next_run": "2026-09-30T00:00:00Z", "tags": ["etl"], "owners": ["data-eng"],
                "file": "etl_nightly.py", "import_errors": false}])
        );
        assert_eq!(
            paths(&transport),
            [
                "dags?order_by=dag_id&dag_id_pattern=etl&tags=etl&tags=orders+team&paused=false&last_dag_run_state=failed&limit=1&offset=0"
            ]
        );
        assert_eq!(outcome.stderr, "[1 of 3; --limit N]\n");
        let sent = &transport.sent()[0];
        assert_eq!(
            sent.authorization.as_deref(),
            Some("Bearer fixture-token-1")
        );
    }

    #[test]
    fn dag_list_pages_past_the_servers_cap_of_100() {
        let page = |n: usize, from: usize| {
            let dags: Vec<Value> = (from..from + n)
                .map(|i| dag(&format!("d{i:03}"), false))
                .collect();
            Answer::json(&json!({"dags": dags, "total_entries": 130}))
        };
        let (outcome, transport) = airflow(
            &["airflow", "dag", "list", "--limit", "150", "--fields", "id"],
            vec![page(100, 0), page(30, 100)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json().as_array().unwrap().len(), 130);
        assert_eq!(
            paths(&transport),
            [
                "dags?order_by=dag_id&limit=100&offset=0",
                "dags?order_by=dag_id&limit=50&offset=100"
            ]
        );
        assert!(outcome.stderr.is_empty(), "{}", outcome.stderr);
    }
}
