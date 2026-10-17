//! `airflow dag diff`: one DAG on two instances (qa against prod), or two
//! DAGs: its source by lines, schedule, catchup, max active runs, params'
//! defaults, tags, owners, paused, and its tasks: added or removed, each
//! one's operator, pool, queue, retries and trigger rule, and its edges.

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Side, command, diff, scoped};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::{Airflow, At, Client, Ref, Want, text};
use crate::source::dag_source;

use super::{detail, details};

#[derive(clap::Args)]
pub struct DagDiffArgs {
    /// One side: [INSTANCE:]DAG (qa:etl_nightly); no prefix is --instance's
    left: String,
    /// The other side, as left
    right: String,
    #[command(flatten)]
    at: At,
    #[command(flatten)]
    diff: DiffArgs,
}

const RULES: Rules = Rules {
    keys: &[("tasks", "task_id"), ("params", "name")],
    noise: &[
        "id",
        "next_run",
        "next_logical_date",
        "last_parsed",
        "version",
        "recent_runs",
    ],
    comments: &["#"],
    ..Rules::PLAIN
};

fn dag_diff(ctx: &Ctx, args: DagDiffArgs) -> Result<Diff> {
    let airflow = Airflow::load(ctx.config())?;
    let side = |raw: &str| -> Result<Side> {
        let (scope, rest) = scoped(raw);
        let wanted = scope.or(args.at.instance.as_deref());
        let (client, id) = airflow.locate(ctx, wanted, rest, Want::Dag, None, None)?;
        let dag = details(&client, &id)?;
        let mut row = serde_json::to_value(detail(&dag, Vec::new()))?;
        row["source"] = dag_source(&client, &dag, &id.dag)?["content"].clone();
        row["tasks"] = tasks(&client, &id)?;
        Ok(Side::new(
            format!("{}:{}", client.instance.name, id.dag),
            row,
        ))
    };
    let (left, right) = (side(&args.left)?, side(&args.right)?);
    diff(ctx, &args.diff, &RULES, left, right)
}

/// The DAG's tasks as its parsed definition has them, edges by name.
fn tasks(client: &Client, id: &Ref) -> Result<Value> {
    let answer = client.get(&format!("{}/tasks", id.dag_path()))?;
    let rows = answer["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|task| {
            let mut downstream: Vec<&str> = task["downstream_task_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            downstream.sort_unstable();
            json!({
                "task_id": task["task_id"],
                // Airflow 3 names it; 2 gives the class.
                "operator": text(&task["operator_name"])
                    .or_else(|| text(&task["class_ref"]["class_name"])),
                "pool": task["pool"],
                "pool_slots": task["pool_slots"],
                "queue": task["queue"],
                "retries": task["retries"],
                "trigger_rule": task["trigger_rule"],
                "downstream": downstream,
            })
        })
        .collect();
    Ok(Value::Array(rows))
}

command! {
    pub DAG_DIFF = ["airflow", "dag", "diff"], Read,
    "Compare a DAG on two instances: its source by lines, schedule and tasks",
    keywords: ["staging", "code", "schedule", "etl"],
    example: "airflow dag diff qa:etl_nightly prod:etl_nightly",
    run: dag_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testing::{TWO, airflow_with, dag, paths};

    fn tasks(retries: i64) -> Answer {
        Answer::json(&json!({"tasks": [
            {"task_id": "extract", "operator_name": "PythonOperator", "pool": "default_pool",
                "pool_slots": 1, "queue": "default", "retries": retries, "trigger_rule": "all_success",
                "downstream_task_ids": ["load"]},
            {"task_id": "load", "operator_name": "PythonOperator", "pool": "default_pool",
                "pool_slots": 1, "queue": "default", "retries": 1, "trigger_rule": "all_success",
                "downstream_task_ids": []},
        ], "total_entries": 2}))
    }

    fn details(schedule: &str) -> Value {
        let mut held = dag("etl_nightly", false);
        held["timetable_summary"] = json!(schedule);
        held
    }

    #[test]
    fn a_dag_on_two_instances_differs_by_schedule_source_lines_and_task_retries() {
        let source = |line: &str| {
            Answer::json(
                &json!({"content": format!("import x\n# a comment\n{line}\nrun()\n"),
                "version_number": 3}),
            )
        };
        let (outcome, transport) = airflow_with(
            TWO,
            &[
                "airflow",
                "dag",
                "diff",
                "qa:etl_nightly",
                "prod:etl_nightly",
            ],
            vec![
                Answer::json(&details("0 1 * * *")),
                source("batch = 500"),
                tasks(0),
                Answer::json(&details("0 0 * * *")),
                source("batch = 5000"),
                tasks(2),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["left"], "qa:etl_nightly");
        assert_eq!(got["as"], json!(["qa=prod"]));
        assert_eq!(
            got["changes"],
            json!([
                {"path": "schedule", "left": "0 1 * * *", "right": "0 0 * * *"},
                {"path": "source", "lines": ["@@ -1,4 +1,4 @@", " import x", " # a comment",
                    "-batch = 500", "+batch = 5000", " run()"]},
                {"path": "tasks.extract.retries", "left": 0, "right": 2},
            ])
        );
        assert_eq!(
            paths(&transport)[..3],
            [
                "https://airflow-qa.contoso.example/api/v2/dags/etl_nightly/details",
                "https://airflow-qa.contoso.example/api/v2/dagSources/etl_nightly",
                "https://airflow-qa.contoso.example/api/v2/dags/etl_nightly/tasks",
            ]
        );
        let (outcome, _) =
            airflow_with(TWO, &["airflow", "dag", "diff", "dev:x", "prod:x"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("qa, prod"), "{}", outcome.stderr);
    }
}
