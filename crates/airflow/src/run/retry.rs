use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::Airflow;

use super::RunIdArgs;
use crate::dag_run::cleared_ids;

#[derive(Debug, Serialize, JsonSchema)]
pub struct Retried {
    id: String,
    /// The task instances cleared to run again.
    cleared: Vec<String>,
}

fn run_retry(ctx: &Ctx, args: RunIdArgs) -> Result<Retried> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = args.locate(&airflow, ctx)?;
    client.writable()?;
    // Airflow 2's run clear takes no only_failed and resets every task, so
    // there the DAG's clearTaskInstances clears the run's failed ones.
    let (path, body) = if client.v1()? {
        (
            format!("{}/clearTaskInstances", id.dag_path()),
            json!({"dag_run_id": id.run, "only_failed": true, "reset_dag_runs": true}),
        )
    } else {
        (
            format!("{}/clear", id.run_path()),
            json!({"only_failed": true}),
        )
    };
    let with = |dry_run: bool| {
        let mut body = body.clone();
        body["dry_run"] = json!(dry_run);
        body
    };
    // The server's own dry run says what a clear would reset; a real clear
    // must send dry_run false, since true is the default.
    let preview = client.preview(&path, with(true))?;
    let cleared = cleared_ids(&preview);
    if cleared.is_empty() {
        ctx.note(format!(
            "[nothing to retry: run {} has no failed or upstream_failed task instances]",
            id.run_id()
        ));
        return Ok(Retried {
            id: id.run_id(),
            cleared,
        });
    }
    if ctx.globals().dry_run {
        ctx.note(format!("[would clear: {}]", cleared.join(", ")));
    }
    client.change(Effect::Write, Method::Post, &path, with(false))?;
    ctx.note(format!(
        "[next: agent-cli airflow run wait {}]",
        id.run_id()
    ));
    Ok(Retried {
        id: id.run_id(),
        cleared,
    })
}

command! {
    pub RUN_RETRY = ["airflow", "run", "retry"], Write,
    "Retry the failed and upstream_failed tasks of a DAG run",
    keywords: ["clear", "rerun", "failed", "again", "resume", "restart"],
    example: "airflow run retry etl_nightly/latest",
    run: run_retry,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RUN, RUN_PATH, airflow, dry_run, run, tasks, ti};

    #[test]
    fn run_retry_previews_on_the_server_then_clears_with_dry_run_false() {
        let preview = tasks(vec![
            ti("load_orders", "failed", 2),
            ti("publish_report", "upstream_failed", 0),
        ]);
        let (plans, stderr) = dry_run(
            &["airflow", "run", "retry", &format!("etl_nightly/{RUN}")],
            vec![preview.clone()],
        );
        assert_eq!(
            plans[0]["url"],
            format!("https://airflow.contoso.example/api/v2/{RUN_PATH}/clear")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"dry_run": false, "only_failed": true})
        );
        assert!(
            stderr.contains(&format!(
                "[would clear: etl_nightly/{RUN}/load_orders, etl_nightly/{RUN}/publish_report]"
            )),
            "{stderr}"
        );

        let (outcome, transport) = airflow(
            &["airflow", "run", "retry", &format!("etl_nightly/{RUN}")],
            vec![preview, Answer::json(&run("queued"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["cleared"].as_array().unwrap().len(), 2);
        let sent = transport.sent();
        assert!(sent[0].method.is_read(), "the preview is a read");
        assert_eq!(
            sent[0].body,
            Some(json!({"dry_run": true, "only_failed": true}))
        );
        assert_eq!(
            sent[1].body,
            Some(json!({"dry_run": false, "only_failed": true}))
        );

        let (outcome, transport) = airflow(
            &["airflow", "run", "retry", &format!("etl_nightly/{RUN}")],
            vec![tasks(vec![])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent().len(),
            1,
            "nothing to clear, nothing written"
        );
        assert!(
            outcome.stderr.contains("nothing to retry"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn airflow_2_retries_a_run_through_the_dags_clear_task_instances() {
        // Airflow 2 lists each map index of a task without its index.
        let preview = tasks(vec![
            ti("load_orders", "failed", 1),
            ti("load_orders", "failed", 1),
        ]);
        let (plans, stderr) = crate::testing::dry_run_with(
            crate::testing::CONFIG_V1,
            &["airflow", "run", "retry", &format!("etl_nightly/{RUN}")],
            vec![preview],
        );
        assert_eq!(
            plans[0]["url"],
            "https://airflow.contoso.example/api/v1/dags/etl_nightly/clearTaskInstances"
        );
        assert_eq!(
            plans[0]["body"],
            json!({"dag_run_id": RUN, "only_failed": true, "reset_dag_runs": true, "dry_run": false})
        );
        assert!(
            stderr.contains(&format!("[would clear: etl_nightly/{RUN}/load_orders]")),
            "{stderr}"
        );
    }
}
