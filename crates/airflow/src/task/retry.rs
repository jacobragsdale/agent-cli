use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Airflow, At, Want};
use crate::dag_run::cleared_ids;

#[derive(clap::Args)]
pub struct TaskRetryArgs {
    /// Task instances of one run: DAG/RUN/TASK[:MAP] (repeat for more)
    #[arg(required = true)]
    tasks: Vec<String>,
    /// The DAG, when the ids leave it out
    #[arg(long)]
    dag: Option<String>,
    /// The run id, when the ids leave it out
    #[arg(long)]
    run: Option<String>,
    /// Clear only these tasks, not the tasks downstream of them
    #[arg(long)]
    no_downstream: bool,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TasksRetried {
    /// DAG/RUN.
    run: String,
    cleared: Vec<String>,
}

fn task_retry(ctx: &Ctx, args: TaskRetryArgs) -> Result<TasksRetried> {
    let airflow = Airflow::load(ctx.config())?;
    let (dag, run) = (args.dag.as_deref(), args.run.as_deref());
    let (client, first) = airflow.locate(
        ctx,
        args.at.instance.as_deref(),
        &args.tasks[0],
        Want::Task,
        dag,
        run,
    )?;
    let mut ids = vec![first];
    for raw in &args.tasks[1..] {
        let (_, mut id) = airflow.find(Some(&client.instance.name), raw, Want::Task, dag, run)?;
        client.resolve(&mut id)?;
        ids.push(id);
    }
    let run_id = ids[0].run_id();
    if let Some(other) = ids.iter().find(|id| id.run_id() != run_id) {
        return Err(Failure::usage(format!(
            "task retry clears tasks of one run, and these name {run_id} and {}",
            other.run_id()
        ))
        .hint("run it once per run")
        .into());
    }
    client.writable()?;
    let task_ids: Vec<Value> = ids
        .iter()
        .map(|id| match id.map {
            Some(map) => json!([id.task, map]),
            None => json!(id.task),
        })
        .collect();
    // Clearing only a failed task leaves its upstream_failed children
    // terminal, so downstream comes along unless asked not to.
    let body = |dry_run: bool| {
        json!({
            "dag_run_id": ids[0].run,
            "task_ids": task_ids,
            "only_failed": false,
            "include_downstream": !args.no_downstream,
            "reset_dag_runs": true,
            "dry_run": dry_run,
        })
    };
    let path = format!("{}/clearTaskInstances", ids[0].dag_path());
    let cleared = cleared_ids(&client.preview(&path, body(true))?);
    if cleared.is_empty() {
        ctx.note("[nothing to clear: no such task instances in that run]");
        return Ok(TasksRetried {
            run: run_id,
            cleared,
        });
    }
    if ctx.globals().dry_run {
        ctx.note(format!("[would clear: {}]", cleared.join(", ")));
    }
    client.change(Effect::Destructive, Method::Post, &path, body(false))?;
    ctx.note(format!("[next: agent-cli airflow run wait {run_id}]"));
    Ok(TasksRetried {
        run: run_id,
        cleared,
    })
}

command! {
    pub TASK_RETRY = ["airflow", "task", "retry"], Destructive,
    "Clear task instances in any state, and their downstream, so they run again",
    keywords: ["clear", "rerun", "downstream", "reset", "again", "one", "restart"],
    example: "airflow task retry etl_nightly/latest/load_orders --yes",
    run: task_retry,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{RUN, airflow, dry_run, ti, tis};

    #[test]
    fn task_retry_clears_the_tasks_and_downstream_after_a_server_preview() {
        let preview = tis(vec![
            ti("load_orders", "failed", 2),
            ti("publish_report", "upstream_failed", 0),
        ]);
        let (plans, stderr) = dry_run(
            &[
                "airflow",
                "task",
                "retry",
                &format!("etl_nightly/{RUN}/load_orders/2"),
                &format!("etl_nightly/{RUN}/load_orders:1"),
            ],
            vec![preview.clone()],
        );
        assert_eq!(
            plans[0]["url"],
            "https://airflow.contoso.example/api/v2/dags/etl_nightly/clearTaskInstances"
        );
        assert_eq!(
            plans[0]["body"],
            json!({"dag_run_id": RUN, "task_ids": ["load_orders", ["load_orders", 1]], "only_failed": false,
                "include_downstream": true, "reset_dag_runs": true, "dry_run": false})
        );
        assert!(
            stderr.contains(&format!(
                "[would clear: etl_nightly/{RUN}/load_orders, etl_nightly/{RUN}/publish_report]"
            )),
            "{stderr}"
        );

        let (outcome, transport) = airflow(
            &[
                "airflow",
                "task",
                "retry",
                &format!("etl_nightly/{RUN}/load_orders"),
                "--no-downstream",
                "--yes",
            ],
            vec![preview, tis(vec![ti("load_orders", "failed", 2)])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert!(sent[0].method.is_read());
        assert_eq!(sent[0].body.as_ref().unwrap()["dry_run"], true);
        assert_eq!(sent[0].body.as_ref().unwrap()["include_downstream"], false);
        assert_eq!(sent[1].body.as_ref().unwrap()["dry_run"], false);

        let (outcome, transport) = airflow(
            &[
                "airflow",
                "task",
                "retry",
                &format!("etl_nightly/{RUN}/load_orders"),
                "etl_nightly/other/load_orders",
                "--yes",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "one run per call: {outcome:?}");
        assert!(transport.sent().is_empty());
        let (outcome, _) = airflow(
            &[
                "airflow",
                "task",
                "retry",
                &format!("etl_nightly/{RUN}/load_orders"),
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "destructive needs --yes: {outcome:?}");
    }
}
