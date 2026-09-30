use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Ado, Kind, list, text};

use super::{is, log_id, timeline};

#[derive(clap::Args)]
pub struct LogsArgs {
    /// The run's id: 8812, #8812 or its web URL
    id: String,
    /// A job's log, by name
    #[arg(long, conflicts_with = "task")]
    job: Option<String>,
    /// A task's log, by name
    #[arg(long)]
    task: Option<String>,
    /// Lines from the end of each log
    #[arg(long, default_value_t = 200)]
    tail: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunLogs {
    run: i64,
    /// Whose logs these are, in the order `text` holds them.
    logs: Vec<String>,
    text: String,
}

/// The logs to read: the named job or task, else every failed task, else
/// the task running now or the last one that wrote anything.
fn chosen_logs(
    id: i64,
    records: &[Value],
    job: Option<&str>,
    task: Option<&str>,
) -> Result<Vec<(String, i64)>> {
    let named = |record: &Value, name: &str| {
        record["name"]
            .as_str()
            .is_some_and(|held| held.eq_ignore_ascii_case(name.trim()))
    };
    let pick =
        |record: &Value, log: Option<i64>| Some((text(&record["name"]).unwrap_or_default(), log?));
    let picked: Vec<(String, i64)> = match (job, task) {
        (Some(job), _) => records
            .iter()
            .filter(|record| is(record, "Job") && named(record, job))
            .filter_map(|record| {
                // A job whose phase holds the log takes it: in most pipelines
                // that log is the whole job's.
                let phase = records
                    .iter()
                    .find(|parent| is(parent, "Phase") && parent["id"] == record["parentId"]);
                pick(record, log_id(record).or_else(|| phase.and_then(log_id)))
            })
            .collect(),
        (None, Some(task)) => records
            .iter()
            .filter(|record| is(record, "Task") && named(record, task))
            .filter_map(|record| pick(record, log_id(record)))
            .collect(),
        (None, None) => {
            let tasks = || {
                records
                    .iter()
                    .filter(|record| is(record, "Task") && log_id(record).is_some())
            };
            let failed: Vec<(String, i64)> = tasks()
                .filter(|record| record["result"].as_str() == Some("failed"))
                .filter_map(|record| pick(record, log_id(record)))
                .collect();
            if failed.is_empty() {
                tasks()
                    .max_by_key(|record| {
                        (
                            record["state"].as_str() == Some("inProgress"),
                            record["startTime"].as_str().map(str::to_owned),
                        )
                    })
                    .and_then(|record| pick(record, log_id(record)))
                    .into_iter()
                    .collect()
            } else {
                failed
            }
        }
    };
    if picked.is_empty() {
        let (kind, name) = match (job, task) {
            (Some(job), _) => ("Job", Some(job)),
            (None, Some(task)) => ("Task", Some(task)),
            (None, None) => ("Task", None),
        };
        let names: Vec<String> = records
            .iter()
            .filter(|record| is(record, kind))
            .filter_map(|record| text(&record["name"]))
            .take(20)
            .collect();
        let message = match name {
            Some(name) => format!(
                "run {id} has no {} named {name:?} with a log",
                kind.to_lowercase()
            ),
            None => format!("run {id} has written no log yet"),
        };
        return Err(Failure::not_found(message)
            .hint(if names.is_empty() {
                format!("agent-cli ado run get {id} --fields status,result")
            } else {
                format!("its {}s: {}", kind.to_lowercase(), names.join(", "))
            })
            .into());
    }
    Ok(picked)
}

fn run_logs(ctx: &Ctx, args: LogsArgs) -> Result<RunLogs> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.id)?;
    let records = timeline(ctx, &ado, id)?;
    let chosen = chosen_logs(id, &records, args.job.as_deref(), args.task.as_deref())?;
    // The line counts say where each tail starts, so a long log is not read
    // whole to keep its last lines.
    let counts = ado.get(ctx, &ado.code(&format!("build/builds/{}/logs", id), ""))?;
    let mut parts = Vec::new();
    for (name, log) in &chosen {
        let lines = list(&counts["value"])
            .iter()
            .find(|entry| entry["id"].as_i64() == Some(*log))
            .and_then(|entry| entry["lineCount"].as_u64())
            .and_then(|count| usize::try_from(count).ok());
        let start = lines.map_or(0, |count| count.saturating_sub(args.tail));
        let url = ado.code(
            &format!("build/builds/{}/logs/{log}", id),
            &format!("startLine={start}"),
        );
        let answer = ado.get(ctx, &url)?;
        let read: Vec<&str> = list(&answer["value"])
            .iter()
            .filter_map(Value::as_str)
            .collect();
        // Whether startLine counts from 0 or 1 is the service's business;
        // the tail is the tail either way.
        let tail = read[read.len().saturating_sub(args.tail)..].join("\n");
        parts.push(if chosen.len() > 1 {
            format!("--- {name} ---\n{tail}")
        } else {
            tail
        });
    }
    Ok(RunLogs {
        run: id,
        logs: chosen.into_iter().map(|(name, _)| name).collect(),
        text: parts.join("\n"),
    })
}

command! {
    pub RUN_LOGS = ["ado", "run", "logs"], Read,
    "Print the tail of a run's logs: failed tasks by default, or a job or task",
    keywords: ["output", "console", "error", "failed", "build", "tail", "trace"],
    example: "ado run logs 1234 --task 'Run tests' --tail 100",
    run: run_logs,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, page, record, timeline, urls};

    #[test]
    fn logs_tail_the_failed_tasks_from_the_line_count() {
        let lines: Vec<String> = (1..=3).map(|n| format!("line {n}")).collect();
        let (outcome, transport) = ado(
            &["ado", "run", "logs", "991", "--tail", "2"],
            vec![
                timeline(),
                page(vec![
                    json!({"id": 5, "lineCount": 900}),
                    json!({"id": 4, "lineCount": 10}),
                ]),
                page(lines.iter().map(|line| json!(line)).collect()),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"run": 991, "logs": ["Run tests"], "text": "line 2\nline 3"})
        );
        assert_eq!(
            urls(&transport)[2],
            format!("{CODE}/build/builds/991/logs/5?startLine=898&api-version=7.1")
        );
    }

    #[test]
    fn a_named_job_takes_its_phases_log_and_an_unknown_one_lists_the_jobs() {
        let (outcome, transport) = ado(
            &["ado", "run", "logs", "991", "--job", "linux"],
            vec![timeline(), page(vec![]), page(vec![json!("job output")])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["text"], "job output");
        assert_eq!(
            urls(&transport)[2],
            format!("{CODE}/build/builds/991/logs/2?startLine=0&api-version=7.1")
        );

        let (outcome, _) = ado(
            &["ado", "run", "logs", "991", "--task", "Deploy"],
            vec![timeline()],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: its tasks: Restore, Run tests"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = ado(&["ado", "run", "logs", "992"], vec![Answer::ok("")]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("has written no log yet"),
            "{}",
            outcome.stderr
        );
        let (outcome, _) = ado(
            &["ado", "run", "logs", "1", "--job", "a", "--task", "b"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn with_nothing_failed_the_logs_are_the_last_tasks() {
        let answer = json!({"records": [
            record("t1", "Task", "Restore", None, Some("succeeded"), 4),
            record("t2", "Task", "Build", None, Some("succeeded"), 6),
        ]});
        let (outcome, _) = ado(
            &["ado", "run", "logs", "991"],
            vec![
                Answer::json(&answer),
                page(vec![]),
                page(vec![json!("built")]),
            ],
        );
        assert_eq!(outcome.json()["logs"], json!(["Build"]));
    }
}
