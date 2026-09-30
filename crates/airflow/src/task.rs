//! Task instances: a run's tasks with their states, one task's tries and
//! what blocks it, its log (the tail and the exception), and clearing tasks
//! to run again. With KubernetesExecutor a task instance's `hostname` is its
//! pod, printed as the k8s id `k8s pod logs` takes.

use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Airflow, At, Client, Ref, Want, note_more, query_value, stamp, text, ti_id};
use crate::run::cleared_ids;

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
/// States a task instance leaves only when something else happens.
const FINISHED: [&str; 5] = ["success", "failed", "skipped", "upstream_failed", "removed"];
/// Elasticsearch and OpenSearch hand a finished log back in pages; no log
/// needs more than this many.
const MOST_PAGES: usize = 20;
/// What `task logs` leaves of the deadline once it has a page.
const MARGIN: Duration = Duration::from_secs(3);

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

// ---------- airflow task list ----------

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

// ---------- airflow task get ----------

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

// ---------- airflow task logs ----------

#[derive(clap::Args)]
pub struct TaskLogsArgs {
    #[command(flatten)]
    id: TaskIdArgs,
    /// How many of the last lines (0 for all)
    #[arg(long, default_value_t = 200)]
    tail: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TaskLogs {
    /// The task instance and the try read.
    id: String,
    state: Option<String>,
    /// The exception that failed it, and where in the DAG's code.
    error: Option<String>,
    lines: usize,
    /// The last lines kept (--tail).
    kept: usize,
    /// False while the task still runs: run it again for more.
    complete: bool,
    /// Where Airflow looked for the log. When it found none, stderr names
    /// the pod's k8s id to read instead.
    sources: Vec<String>,
    text: String,
}

fn task_logs(ctx: &Ctx, args: TaskLogsArgs) -> Result<TaskLogs> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = args.id.locate(&airflow, ctx)?;
    // The try the id names, or the latest; either way its state and host.
    let ti = match id.attempt {
        Some(attempt) => client.get(&format!("{}/tries/{attempt}", id.ti_path()))?,
        None => client.get(&id.ti_path())?,
    };
    let attempt = id.attempt.or(ti["try_number"].as_i64()).unwrap_or_default();
    let state = text(&ti["state"]);
    let done = finished(state.as_deref());
    let path = format!(
        "{}/taskInstances/{}/logs/{attempt}",
        id.run_path(),
        crate::client::segment(&id.task)
    );
    let base = format!(
        "{path}?full_content=true&map_index={}",
        id.map.unwrap_or(-1)
    );
    let mut content: Vec<Value> = Vec::new();
    let mut url = base.clone();
    let mut complete = true;
    for page in 1.. {
        let answer = client.get(&url)?;
        content.extend(answer["content"].as_array().cloned().unwrap_or_default());
        let Some(token) = text(&answer["continuation_token"]) else {
            break;
        };
        // A running task's token only ever means "ask again later".
        let room = ctx.deadline().saturating_duration_since(Instant::now()) > MARGIN;
        if !done || !room || page >= MOST_PAGES {
            complete = false;
            break;
        }
        url = format!("{base}&token={}", query_value(&token));
    }
    if !done {
        complete = false;
        ctx.note(format!(
            "[{} is still {}: this is the log so far]",
            ti_id(&ti, true),
            state.as_deref().unwrap_or("unfinished")
        ));
    }
    let rendered = render(&content);
    let lines = rendered.lines.len();
    let kept = if args.tail == 0 {
        lines
    } else {
        args.tail.min(lines)
    };
    if rendered.lines.is_empty() && attempt > 0 {
        let pod = client.instance.pod(ti["hostname"].as_str());
        let why = rendered
            .sources
            .iter()
            .find(|source| !source.trim().is_empty())
            .map_or("it named no source", String::as_str);
        let why: String = why.chars().take(160).collect();
        ctx.note(match &pod {
            Some(pod) => format!(
                "[Airflow has no log for this try ({why}). The pod's own log, while the pod exists: agent-cli k8s pod logs {pod} --tail {}, and its events: agent-cli k8s event list --pod {pod}]",
                if args.tail == 0 { 200 } else { args.tail }
            ),
            None => format!(
                "[Airflow has no log for this try ({why}); remote logging may be off. With k8s_scope on the instance, task get prints the pod's k8s id]"
            ),
        });
    }
    Ok(TaskLogs {
        id: format!("{}/{attempt}", ti_id(&ti, false)),
        state,
        error: rendered.error,
        lines,
        kept,
        complete,
        sources: rendered.sources,
        text: rendered.lines[lines - kept..].join("\n"),
    })
}

command! {
    pub TASK_LOGS = ["airflow", "task", "logs"], Read,
    "Read the tail of a task instance's log, with the exception that failed it",
    keywords: ["show", "log", "output", "error", "traceback", "exception", "failed", "why"],
    example: "airflow task logs etl_nightly/latest/load_orders --tail 50",
    run: task_logs,
}

/// A log as Airflow's UI shows it, one line per message.
struct Rendered {
    sources: Vec<String>,
    lines: Vec<String>,
    error: Option<String>,
}

/// Keys every message carries that say nothing a line needs.
const QUIET: [&str; 13] = [
    "timestamp",
    "event",
    "level",
    "logger",
    "error_detail",
    "ti_id",
    "dag_id",
    "task_id",
    "run_id",
    "try_number",
    "map_index",
    "filename",
    "lineno",
];

/// Airflow's structured messages (`{timestamp, level, logger, event,
/// error_detail?, …}`) or plain strings as lines: the `::group::Log message
/// source details` block becomes `sources`, an `error_detail` becomes a
/// Python-style traceback, and `error` is the last exception with its
/// innermost frame outside installed packages (the UI's user-code rule).
fn render(content: &[Value]) -> Rendered {
    let mut out = Rendered {
        sources: Vec::new(),
        lines: Vec::new(),
        error: None,
    };
    let mut in_sources = false;
    for message in content {
        let Some(fields) = message.as_object() else {
            out.lines.extend(
                message
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_owned),
            );
            continue;
        };
        let event = fields
            .get("event")
            .map(|event| {
                event
                    .as_str()
                    .map_or_else(|| event.to_string(), str::to_owned)
            })
            .unwrap_or_default();
        match event.as_str() {
            "::group::Log message source details" => in_sources = true,
            "::endgroup::" => in_sources = false,
            _ if in_sources => out.sources.push(event),
            _ => {
                let mut line = String::new();
                for (key, suffix) in [("timestamp", " "), ("level", " "), ("logger", ": ")] {
                    if let Some(value) = fields.get(key).and_then(Value::as_str) {
                        let value = if key == "level" {
                            value.to_uppercase()
                        } else {
                            value.to_owned()
                        };
                        line.push_str(&value);
                        line.push_str(suffix);
                    }
                }
                line.push_str(&event);
                for (key, value) in fields
                    .iter()
                    .filter(|(key, _)| !QUIET.contains(&key.as_str()))
                {
                    let value = value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned);
                    line.push_str(&format!(" {key}={value}"));
                }
                out.lines.extend(line.lines().map(str::to_owned));
                if let Some(stacks) = fields.get("error_detail").and_then(Value::as_array) {
                    // structlog lists the raised exception first and its
                    // causes after; Python prints the causes first.
                    for stack in stacks.iter().rev() {
                        out.lines
                            .push("Traceback (most recent call last):".to_owned());
                        for frame in stack["frames"].as_array().into_iter().flatten() {
                            out.lines.push(format!(
                                "  File \"{}\", line {}, in {}",
                                frame["filename"].as_str().unwrap_or("?"),
                                frame["lineno"],
                                frame["name"].as_str().unwrap_or("?")
                            ));
                        }
                        out.lines.push(exception(stack));
                    }
                    if let Some(root) = stacks.last() {
                        out.error = Some(summary(root));
                    }
                }
            }
        }
    }
    if out.error.is_none() {
        out.error = last_traceback(&out.lines);
    }
    out
}

fn exception(stack: &Value) -> String {
    let kind = stack["exc_type"].as_str().unwrap_or("Exception");
    match stack["exc_value"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        Some(value) => format!("{kind}: {value}"),
        None => kind.to_owned(),
    }
}

/// `ValueError: … (at dags/etl.py:42 in load_orders)`: the innermost frame
/// that is not an installed package, which is where the DAG's code failed.
fn summary(stack: &Value) -> String {
    let frames: Vec<&Value> = stack["frames"].as_array().into_iter().flatten().collect();
    let installed = |frame: &&&Value| {
        let file = frame["filename"].as_str().unwrap_or_default();
        file.contains("site-packages") || file.contains("dist-packages")
    };
    let frame = frames
        .iter()
        .rev()
        .find(|frame| !installed(frame))
        .or(frames.last());
    match frame {
        Some(frame) => format!(
            "{} (at {}:{} in {})",
            exception(stack),
            frame["filename"].as_str().unwrap_or("?"),
            frame["lineno"],
            frame["name"].as_str().unwrap_or("?")
        ),
        None => exception(stack),
    }
}

/// A plain-text log's last traceback, as its exception line.
fn last_traceback(lines: &[String]) -> Option<String> {
    let start = lines
        .iter()
        .rposition(|line| line.contains("Traceback (most recent call last)"))?;
    lines[start + 1..]
        .iter()
        .find(|line| !line.trim().is_empty() && !line.starts_with(char::is_whitespace))
        .map(|line| line.trim().to_owned())
}

// ---------- airflow task retry ----------

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
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use super::render;
    use crate::run::tests::{RUN, RUN_PATH, ti};
    use crate::testkit::{airflow, airflow_with, dry_run, paths};

    fn tis(items: Vec<Value>) -> Answer {
        Answer::json(&json!({"total_entries": items.len(), "task_instances": items}))
    }

    fn log(content: Value) -> Answer {
        Answer::json(&json!({"content": content, "continuation_token": null}))
    }

    fn failing_log() -> Value {
        json!([
            {"event": "::group::Log message source details", "timestamp": null},
            {"event": "Found logs in s3://contoso-airflow-logs/dag_id=etl_nightly/run_id=x/task_id=load_orders/attempt=2.log"},
            {"event": "::endgroup::"},
            {"timestamp": "2026-09-29T00:41:07.120511Z", "level": "info", "logger": "task", "event": "Loading orders", "day": "2026-09-28", "ti_id": "0199"},
            {"timestamp": "2026-09-29T00:43:55.004Z", "level": "error", "logger": "task", "event": "Task failed with exception",
             "error_detail": [{"exc_type": "ValueError", "exc_value": "order 88123 has no customer_id", "syntax_error": null,
               "is_cause": false, "exc_notes": [], "frames": [
                 {"filename": "/home/airflow/.local/lib/python3.12/site-packages/airflow/sdk/execution_time/task_runner.py", "lineno": 875, "name": "run"},
                 {"filename": "/opt/airflow/dags/etl_nightly.py", "lineno": 42, "name": "load_orders"},
                 {"filename": "/home/airflow/.local/lib/python3.12/site-packages/contoso/rows.py", "lineno": 7, "name": "check"}]}]}
        ])
    }

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

    #[test]
    fn task_logs_tails_the_log_and_extracts_the_exception_in_the_dags_code() {
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "task",
                "logs",
                &format!("etl_nightly/{RUN}/load_orders/2"),
                "--tail",
                "3",
            ],
            vec![
                Answer::json(&ti("load_orders", "failed", 2)),
                log(failing_log()),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["error"],
            "ValueError: order 88123 has no customer_id (at /opt/airflow/dags/etl_nightly.py:42 in load_orders)"
        );
        assert_eq!(got["id"], format!("etl_nightly/{RUN}/load_orders/2"));
        assert_eq!(
            (got["lines"].as_u64(), got["kept"].as_u64()),
            (Some(7), Some(3))
        );
        assert_eq!(got["complete"], true);
        assert!(
            got["sources"][0]
                .as_str()
                .unwrap()
                .starts_with("Found logs in s3://")
        );
        assert_eq!(
            got["text"],
            "  File \"/opt/airflow/dags/etl_nightly.py\", line 42, in load_orders\n  File \"/home/airflow/.local/lib/python3.12/site-packages/contoso/rows.py\", line 7, in check\nValueError: order 88123 has no customer_id"
        );
        assert_eq!(
            paths(&transport),
            [
                format!("{RUN_PATH}/taskInstances/load_orders/tries/2"),
                format!(
                    "{RUN_PATH}/taskInstances/load_orders/logs/2?full_content=true&map_index=-1"
                ),
            ]
        );
    }

    #[test]
    fn a_log_airflow_cannot_serve_names_the_pod_to_read_instead() {
        let content = json!([
            {"event": "::group::Log message source details"},
            {"event": "Could not read served logs: HTTPConnectionPool(host='etl-nightly-load-orders-q8x1k2vz', port=8793): Max retries exceeded"},
            {"event": "::endgroup::"}
        ]);
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "task",
                "logs",
                &format!("etl_nightly/{RUN}/load_orders"),
            ],
            vec![Answer::json(&ti("load_orders", "failed", 2)), log(content)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["lines"], 0);
        assert!(
            outcome.stderr.contains("agent-cli k8s pod logs prod/web/etl-nightly-load-orders-q8x1k2vz --tail 200, and its events: agent-cli k8s event list --pod prod/web/etl-nightly-load-orders-q8x1k2vz]"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("(Could not read served logs"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            paths(&transport)[1],
            format!("{RUN_PATH}/taskInstances/load_orders/logs/2?full_content=true&map_index=-1")
        );
    }

    #[test]
    fn a_running_task_gets_one_page_and_a_mapped_one_sends_its_index() {
        let running = ti("load_orders", "running", 1);
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "task",
                "logs",
                &format!("etl_nightly/{RUN}/load_orders:3"),
            ],
            vec![
                Answer::json(&running),
                Answer::json(
                    &json!({"content": ["plain line 1", "plain line 2"], "continuation_token": "tok-abc"}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["complete"], false);
        assert_eq!(got["text"], "plain line 1\nplain line 2");
        assert!(
            outcome
                .stderr
                .contains("is still running: this is the log so far"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            paths(&transport),
            [
                format!("{RUN_PATH}/taskInstances/load_orders/3"),
                format!(
                    "{RUN_PATH}/taskInstances/load_orders/logs/1?full_content=true&map_index=3"
                ),
            ]
        );
    }

    #[test]
    fn a_paging_remote_log_is_followed_while_the_task_is_finished() {
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "task",
                "logs",
                &format!("etl_nightly/{RUN}/load_orders"),
                "--tail",
                "0",
            ],
            vec![
                Answer::json(&ti("load_orders", "success", 1)),
                Answer::json(&json!({"content": ["page one"], "continuation_token": "tok/1"})),
                Answer::json(&json!({"content": ["page two"], "continuation_token": null})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["text"], "page one\npage two");
        assert!(
            paths(&transport)[2].ends_with("map_index=-1&token=tok%2F1"),
            "{:?}",
            paths(&transport)
        );
    }

    #[test]
    fn plain_text_logs_still_yield_the_last_exception() {
        let rendered = render(&[json!(
            "[2026-09-29, 00:43:55 UTC] {taskinstance.py:3311} ERROR - Task failed\nTraceback (most recent call last):\n  File \"/opt/airflow/dags/etl.py\", line 9, in load\n    raise KeyError('customer_id')\nKeyError: 'customer_id'\n[2026-09-29, 00:43:56 UTC] INFO - Marking task as FAILED."
        )]);
        assert_eq!(rendered.error.as_deref(), Some("KeyError: 'customer_id'"));
        assert_eq!(rendered.lines.len(), 6);
    }

    #[test]
    fn a_huge_log_keeps_its_tail_within_the_output_guard() {
        let lines: Vec<Value> = (0..5000)
            .map(|i| json!({"timestamp": "2026-09-29T00:41:07.1Z", "level": "info", "logger": "task", "event": format!("row {i} loaded")}))
            .collect();
        let (outcome, _) = airflow(
            &[
                "airflow",
                "task",
                "logs",
                &format!("etl_nightly/{RUN}/load_orders"),
                "--tail",
                "0",
            ],
            vec![
                Answer::json(&ti("load_orders", "success", 1)),
                log(Value::Array(lines)),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome.stdout.len() < 13_000,
            "{} bytes",
            outcome.stdout.len()
        );
        assert!(
            outcome.stdout.contains("row 4999 loaded"),
            "the tail survives the guard"
        );
    }

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
