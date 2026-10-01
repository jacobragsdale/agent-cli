use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Airflow, query_value, text, ti_id};
use crate::source::failing_line;

use super::{TaskIdArgs, finished};

/// Elasticsearch and OpenSearch hand a finished log back in pages; no log
/// needs more than this many.
const MOST_PAGES: usize = 20;
/// What `task logs` leaves of the deadline once it has a page.
const MARGIN: Duration = Duration::from_secs(3);

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
    /// DAG:LINE, the innermost frame of the traceback in the DAG's own file:
    /// what source get takes.
    at: Option<String>,
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
    // The DagBag line names the file the task ran from; without it, the
    // DAG's path in its bundle costs one more read, made only on a failure.
    let at = rendered
        .error
        .as_ref()
        .and_then(|_| {
            dag_file(&rendered.lines).or_else(|| {
                let dag = client.get(&id.dag_path()).ok()?;
                text(&dag["relative_fileloc"])
            })
        })
        .and_then(|file| failing_line(rendered.lines.iter().map(String::as_str), &file))
        .map(|line| format!("{}:{line}", id.dag));
    if let Some(at) = &at {
        ctx.note(format!(
            "[the failing line in the DAG's code: agent-cli airflow source get {at}]"
        ));
    }
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
        at,
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
    keywords: ["show", "log", "output", "error", "traceback", "exception", "failed", "why", "line", "code", "raised"],
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

/// The DAG file the task's DagBag filled from, as Airflow logs it at info.
fn dag_file(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|line| {
        let (_, rest) = line.split_once("Filling up the DagBag from ")?;
        rest.split_whitespace().next().map(str::to_owned)
    })
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

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use super::render;
    use crate::testing::{RUN, RUN_PATH, airflow, dag, paths, ti};

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
                Answer::json(&dag("etl_nightly", false)),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["error"],
            "ValueError: order 88123 has no customer_id (at /opt/airflow/dags/etl_nightly.py:42 in load_orders)"
        );
        assert_eq!(
            got["at"], "etl_nightly:42",
            "the DAG's file, by its relative_fileloc"
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
                "dags/etl_nightly".to_owned(),
            ]
        );
    }

    #[test]
    fn at_is_the_innermost_frame_in_the_file_the_dagbag_filled_from() {
        let content = json!([
            "[2026-09-29, 00:41:06 UTC] {dagbag.py:588} INFO - Filling up the DagBag from /opt/airflow/dags/team/etl.py",
            "Traceback (most recent call last):\n  File \"/opt/airflow/dags/team/etl.py\", line 9, in load\n    check(row)\n  File \"/opt/airflow/dags/team/etl.py\", line 30, in check\n    common.require(row, 'customer_id')\n  File \"/opt/airflow/dags/team/common.py\", line 4, in require\n    raise KeyError(key)\nKeyError: 'customer_id'"
        ]);
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "task",
                "logs",
                &format!("etl_nightly/{RUN}/load_orders"),
            ],
            vec![Answer::json(&ti("load_orders", "failed", 1)), log(content)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["at"], "etl_nightly:30");
        assert_eq!(transport.sent().len(), 2, "no read for the DAG's path");
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
}
