//! Task instances: a run's tasks with their states, one task's tries and
//! what blocks it, its log (the tail and the exception), and clearing tasks
//! to run again. With KubernetesExecutor a task instance's `hostname` is its
//! pod, printed as the k8s id `k8s pod logs` takes.

pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod logs;
pub(crate) mod retry;

use agent_cli_core::Ctx;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Airflow, At, Client, Ref, Want, stamp, text, ti_id};

/// States a task instance leaves only when something else happens.
const FINISHED: [&str; 5] = ["success", "failed", "skipped", "upstream_failed", "removed"];

fn finished(state: Option<&str>) -> bool {
    state.is_some_and(|state| FINISHED.contains(&state))
}

/// The positional the one-task commands take, and its leading pieces as
/// flags.
#[derive(clap::Args)]
pub struct TaskIdArgs {
    /// The task instance: DAG/RUN/TASK[:MAP][/TRY], or its Airflow UI URL
    task: String,
    /// The DAG, when the id leaves it out
    #[arg(long)]
    dag: Option<String>,
    /// The run id, when the id leaves it out
    #[arg(long)]
    run: Option<String>,
    #[command(flatten)]
    at: At,
}

impl TaskIdArgs {
    fn locate<'a>(&self, airflow: &'a Airflow, ctx: &'a Ctx) -> Result<(Client<'a>, Ref)> {
        airflow.locate(
            ctx,
            self.at.instance.as_deref(),
            &self.task,
            Want::Task,
            self.dag.as_deref(),
            self.run.as_deref(),
        )
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TaskRow {
    /// DAG/RUN/TASK[:MAP]/TRY: what task get, logs and retry take.
    id: String,
    state: Option<String>,
    try_number: Option<i64>,
    max_tries: Option<i64>,
    start: Option<String>,
    end: Option<String>,
    /// Seconds.
    duration: Option<i64>,
    operator: Option<String>,
    /// Where it ran; with KubernetesExecutor, the pod's name.
    hostname: Option<String>,
}

fn task_row(ti: &Value) -> TaskRow {
    TaskRow {
        id: ti_id(ti, true),
        state: text(&ti["state"]),
        try_number: ti["try_number"].as_i64(),
        max_tries: ti["max_tries"].as_i64(),
        start: stamp(&ti["start_date"]),
        end: stamp(&ti["end_date"]),
        duration: ti["duration"]
            .as_f64()
            .map(|seconds| seconds.round() as i64),
        operator: text(&ti["operator"]),
        hostname: text(&ti["hostname"]),
    }
}
