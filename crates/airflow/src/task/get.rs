use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Airflow, stamp, text};

use super::{TaskIdArgs, finished, task_row};

#[derive(clap::Args)]
pub struct TaskGetArgs {
    #[command(flatten)]
    id: TaskIdArgs,
    /// Add the rendered template fields (large)
    #[arg(long)]
    rendered: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TaskDetail {
    id: String,
    state: Option<String>,
    try_number: Option<i64>,
    max_tries: Option<i64>,
    start: Option<String>,
    end: Option<String>,
    duration: Option<i64>,
    operator: Option<String>,
    executor: Option<String>,
    queue: Option<String>,
    pool: Option<String>,
    hostname: Option<String>,
    /// The k8s id of its pod (scope/namespace/name): what k8s pod logs takes.
    pod: Option<String>,
    note: Option<String>,
    tries: Vec<Try>,
    /// Why an unfinished task does not run yet.
    blocked_by: Vec<Blocker>,
    rendered_fields: Option<Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Try {
    try_number: Option<i64>,
    state: Option<String>,
    start: Option<String>,
    end: Option<String>,
    hostname: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Blocker {
    name: String,
    reason: String,
}

fn task_get(ctx: &Ctx, args: TaskGetArgs) -> Result<TaskDetail> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = args.id.locate(&airflow, ctx)?;
    let ti = client.get(&id.ti_path())?;
    let tries = client.get(&format!("{}/tries", id.ti_path()))?;
    let state = text(&ti["state"]);
    let blocked_by = if finished(state.as_deref()) {
        Vec::new()
    } else {
        let answer = client.get(&format!("{}/dependencies", id.ti_path()))?;
        answer["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|dependency| Blocker {
                name: dependency["name"].as_str().unwrap_or_default().to_owned(),
                reason: dependency["reason"].as_str().unwrap_or_default().to_owned(),
            })
            .collect()
    };
    let row = task_row(&ti);
    Ok(TaskDetail {
        pod: client.instance.pod(row.hostname.as_deref()),
        id: row.id,
        state,
        try_number: row.try_number,
        max_tries: row.max_tries,
        start: row.start,
        end: row.end,
        duration: row.duration,
        operator: row.operator,
        executor: text(&ti["executor"]),
        queue: text(&ti["queue"]),
        pool: text(&ti["pool"]),
        hostname: row.hostname,
        note: text(&ti["note"]),
        tries: tries["task_instances"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|attempt| Try {
                try_number: attempt["try_number"].as_i64(),
                state: text(&attempt["state"]),
                start: stamp(&attempt["start_date"]),
                end: stamp(&attempt["end_date"]),
                hostname: text(&attempt["hostname"]),
            })
            .collect(),
        blocked_by,
        rendered_fields: args.rendered.then(|| ti["rendered_fields"].clone()),
    })
}

command! {
    pub TASK_GET = ["airflow", "task", "get"], Read,
    "Show a task instance: its tries, its pod, and what blocks it if it is stuck",
    keywords: ["stuck", "queued", "dependencies", "attempts", "pod", "hostname", "blocked", "why"],
    example: "airflow task get etl_nightly/latest/load_orders --fields state,blocked_by,tries,pod",
    run: task_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testing::{RUN, RUN_PATH, airflow, airflow_with, paths, ti, tis};

    #[test]
    fn task_get_prints_the_pod_as_the_k8s_id_and_what_blocks_a_queued_task() {
        let history = tis(vec![
            ti("load_orders", "failed", 1),
            ti("load_orders", "failed", 2),
        ]);
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "task",
                "get",
                &format!("etl_nightly/{RUN}/load_orders/2"),
            ],
            vec![Answer::json(&ti("load_orders", "failed", 2)), history],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["pod"], "prod/web/etl-nightly-load-orders-q8x1k2vz");
        assert_eq!(got["tries"].as_array().unwrap().len(), 2);
        assert!(got.get("rendered_fields").is_none() && got.get("blocked_by").is_none());
        assert_eq!(
            paths(&transport),
            [
                format!("{RUN_PATH}/taskInstances/load_orders"),
                format!("{RUN_PATH}/taskInstances/load_orders/tries")
            ]
        );

        let mut queued = ti("load_orders", "queued", 0);
        queued["hostname"] = Value::Null;
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "task",
                "get",
                "load_orders:3",
                "--dag",
                "etl_nightly",
                "--run",
                RUN,
                "--rendered",
            ],
            vec![
                Answer::json(&queued),
                tis(vec![]),
                Answer::json(
                    &json!({"dependencies": [{"name": "Pool Slots Available", "reason": "Not scheduling since there are 0 open slots in pool default_pool"}]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["blocked_by"][0]["name"], "Pool Slots Available");
        assert!(got.get("pod").is_none(), "no hostname, no pod");
        assert_eq!(got["rendered_fields"]["op_kwargs"]["day"], "2026-09-28");
        assert_eq!(
            paths(&transport)[2],
            format!("{RUN_PATH}/taskInstances/load_orders/3/dependencies")
        );

        let no_k8s = "[[airflow.instance]]\nname = \"prod\"\nbase_url = \"https://airflow.contoso.example\"\ntoken_env = \"AIRFLOW_TOKEN\"\n";
        let (outcome, _) = airflow_with(
            no_k8s,
            &[
                "airflow",
                "task",
                "get",
                &format!("etl_nightly/{RUN}/load_orders"),
            ],
            vec![Answer::json(&ti("load_orders", "failed", 2)), tis(vec![])],
        );
        assert!(
            outcome.json().get("pod").is_none(),
            "no k8s_scope, no pod: {outcome:?}"
        );
    }

    #[test]
    fn a_mapped_task_without_its_index_is_exit_2_hinting_the_list() {
        let (outcome, _) = airflow(
            &[
                "airflow",
                "task",
                "get",
                &format!("etl_nightly/{RUN}/load_orders"),
            ],
            vec![Answer::status(
                404,
                r#"{"detail":"Task instance is mapped, add the map_index value to the URL"}"#,
            )],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("hint: name the mapped task as TASK:N; its map indexes: agent-cli airflow task list DAG/RUN"), "{}", outcome.stderr);
    }
}
