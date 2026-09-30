//! DAG runs: history, a run's status with its task summary, triggering,
//! a bounded wait, and re-running what failed.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Effect, Exit, Failure, Method, When, command, redact_value, status_of};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::client::{
    Airflow, At, Client, Ref, Want, note_more, query_value, seconds, stamp, text, ti_id,
};
use crate::dag::RUN_STATES;

/// How `run wait` starts polling, and the most it lets a poll wait.
const FIRST_POLL: Duration = Duration::from_secs(2);
const LONGEST_POLL: Duration = Duration::from_secs(10);
/// What a wait leaves of the deadline for its last poll and its answer.
const MARGIN: Duration = Duration::from_secs(2);
/// `run get` counts at most this many task instances.
const MOST_TASKS: usize = 1000;
const RUN_TYPES: [&str; 4] = ["scheduled", "manual", "backfill", "asset_triggered"];

/// The positional every run command takes, and its leading piece as a flag.
#[derive(clap::Args)]
pub struct RunIdArgs {
    /// The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
    run: String,
    /// The DAG, when RUN is a bare run id
    #[arg(long)]
    dag: Option<String>,
    #[command(flatten)]
    at: At,
}

impl RunIdArgs {
    fn locate<'a>(&self, airflow: &'a Airflow, ctx: &'a Ctx) -> Result<(Client<'a>, Ref)> {
        airflow.locate(
            ctx,
            self.at.instance.as_deref(),
            &self.run,
            Want::Run,
            self.dag.as_deref(),
            None,
        )
    }
}

// ---------- airflow run list ----------

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
    /// Only runs due at or after this (their run_after)
    #[arg(long)]
    since: Option<When>,
    /// Only runs due at or before this
    #[arg(long)]
    until: Option<When>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunRow {
    /// DAG/RUN: what run get, wait, retry and task list take.
    id: String,
    state: Option<String>,
    /// scheduled, manual, backfill or asset_triggered.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// When it was due.
    run_after: Option<String>,
    logical_date: Option<String>,
    start: Option<String>,
    end: Option<String>,
    /// Seconds.
    duration: Option<i64>,
    triggered_by: Option<String>,
}

fn run_row(run: &Value) -> RunRow {
    RunRow {
        id: format!(
            "{}/{}",
            run["dag_id"].as_str().unwrap_or_default(),
            run["dag_run_id"].as_str().unwrap_or_default()
        ),
        state: text(&run["state"]),
        kind: text(&run["run_type"]),
        run_after: stamp(&run["run_after"]),
        logical_date: stamp(&run["logical_date"]),
        start: stamp(&run["start_date"]),
        end: stamp(&run["end_date"]),
        duration: seconds(&run["start_date"], &run["end_date"]),
        triggered_by: text(&run["triggered_by"]),
    }
}

fn run_list(ctx: &Ctx, args: RunListArgs) -> Result<Vec<RunRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    // `~` is every DAG.
    let dag = args
        .dag
        .as_deref()
        .map_or_else(|| "~".to_owned(), crate::client::segment);
    let mut query = vec!["order_by=-run_after".to_owned()];
    query.extend(args.state.iter().map(|state| format!("state={state}")));
    query.extend(args.kind.iter().map(|kind| format!("run_type={kind}")));
    if let Some(since) = args.since {
        query.push(format!("run_after_gte={}", query_value(&since.utc())));
    }
    if let Some(until) = args.until {
        query.push(format!("run_after_lte={}", query_value(&until.utc())));
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

// ---------- airflow run get ----------

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunDetail {
    id: String,
    state: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    run_after: Option<String>,
    logical_date: Option<String>,
    start: Option<String>,
    end: Option<String>,
    duration: Option<i64>,
    triggered_by: Option<String>,
    /// Who triggered it (Airflow 3.1+).
    user: Option<String>,
    conf: Value,
    note: Option<String>,
    dag_version: Option<i64>,
    /// Task instances by state.
    tasks: BTreeMap<String, usize>,
    /// The failed task instances (root causes; upstream_failed only counts):
    /// ids task logs takes.
    failed: Vec<String>,
}

fn run_get(ctx: &Ctx, args: RunIdArgs) -> Result<RunDetail> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = args.locate(&airflow, ctx)?;
    let run = client.get(&id.run_path())?;
    let (tasks, total) = client.list(
        &format!("{}/taskInstances", id.run_path()),
        "",
        "task_instances",
        MOST_TASKS,
    )?;
    if total.is_some_and(|total| total > tasks.len()) {
        ctx.note(format!(
            "[counted the first {} of {} task instances]",
            tasks.len(),
            total.unwrap_or_default()
        ));
    }
    let mut counts = BTreeMap::new();
    for task in &tasks {
        let state = task["state"].as_str().unwrap_or("none").to_owned();
        *counts.entry(state).or_default() += 1;
    }
    let row = run_row(&run);
    Ok(RunDetail {
        id: row.id,
        state: row.state,
        kind: row.kind,
        run_after: row.run_after,
        logical_date: row.logical_date,
        start: row.start,
        end: row.end,
        duration: row.duration,
        triggered_by: row.triggered_by,
        user: text(&run["triggering_user_name"]),
        // Airflow masks secrets in logs and rendered fields, but not here.
        conf: redact_value(run["conf"].clone()),
        note: text(&run["note"]),
        dag_version: run["dag_versions"]
            .as_array()
            .and_then(|versions| versions.last())
            .and_then(|version| version["version_number"].as_i64()),
        tasks: counts,
        failed: failed_ids(&tasks),
    })
}

/// The failed task instances, with their try, so they paste into task logs.
fn failed_ids(tasks: &[Value]) -> Vec<String> {
    tasks
        .iter()
        .filter(|task| task["state"] == "failed")
        .map(|task| ti_id(task, true))
        .collect()
}

command! {
    pub RUN_GET = ["airflow", "run", "get"], Read,
    "Show a DAG run's state, task counts by state, and the tasks that failed",
    keywords: ["status", "state", "progress", "summary", "why", "failed", "latest"],
    example: "airflow run get etl_nightly/latest --fields id,state,tasks,failed",
    run: run_get,
}

// ---------- airflow run create ----------

#[derive(clap::Args)]
pub struct RunCreateArgs {
    /// The DAG: its id, or its Airflow UI URL
    dag: String,
    /// The run's conf: a JSON object, or - to read it from stdin
    #[arg(long)]
    conf: Option<String>,
    /// RFC 3339 or now; a DAG that templates {{ ds }} needs one (none by default)
    #[arg(long)]
    logical_date: Option<String>,
    /// A run id of your own; Airflow makes a manual__ one otherwise
    #[arg(long)]
    run_id: Option<String>,
    /// A note on the run
    #[arg(long)]
    note: Option<String>,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunCreated {
    /// DAG/RUN: what run wait takes.
    id: String,
    state: Option<String>,
    run_after: Option<String>,
    logical_date: Option<String>,
    conf: Value,
}

fn run_create(ctx: &Ctx, args: RunCreateArgs) -> Result<RunCreated> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = airflow.locate(
        ctx,
        args.at.instance.as_deref(),
        &args.dag,
        Want::Dag,
        None,
        None,
    )?;
    client.writable()?;
    let conf = match args.conf.as_deref() {
        None => json!({}),
        Some(raw) => {
            let raw = if raw == "-" {
                std::io::read_to_string(std::io::stdin())?
            } else {
                raw.to_owned()
            };
            serde_json::from_str::<Map<String, Value>>(&raw)
                .map(Value::Object)
                .map_err(|error| Failure::usage(format!("--conf is not a JSON object: {error}")))?
        }
    };
    // Since 3.0 a REST trigger without one has no data interval, so a DAG
    // that templates {{ ds }} fails; null is still the API's own default.
    let logical_date = match args.logical_date.as_deref() {
        None => Value::Null,
        Some(raw) => raw
            .parse::<When>()
            .map(|when| Value::String(when.utc()))
            .map_err(|why| Failure::usage(format!("--logical-date: {why}")))?,
    };
    let dag = client.get(&id.dag_path())?;
    if dag["is_paused"].as_bool() == Some(true) {
        ctx.note(format!(
            "[DAG {} is paused: the run stays queued until agent-cli airflow dag update {} --paused false]",
            id.dag, id.dag
        ));
    }
    let mut body = json!({"logical_date": logical_date, "conf": conf});
    if let Some(run_id) = &args.run_id {
        body["dag_run_id"] = json!(run_id);
    }
    if let Some(note) = &args.note {
        body["note"] = json!(note);
    }
    let run = client
        .change(
            Effect::Write,
            Method::Post,
            &format!("{}/dagRuns", id.dag_path()),
            body,
        )
        .map_err(|error| match status_of(&error) {
            Some(400) if format!("{error:#}").contains("import errors") => {
                refine(error, "agent-cli airflow import-error list")
            }
            Some(409) => refine(
                error,
                &format!("agent-cli airflow run list --dag {}", id.dag),
            ),
            _ => error,
        })?;
    let row = run_row(&run);
    ctx.note(format!("[next: agent-cli airflow run wait {}]", row.id));
    Ok(RunCreated {
        id: row.id,
        state: row.state,
        run_after: row.run_after,
        logical_date: row.logical_date,
        conf: redact_value(run["conf"].clone()),
    })
}

/// The same failure with a better next step.
fn refine(error: anyhow::Error, hint: &str) -> anyhow::Error {
    match error.downcast::<Failure>() {
        Ok(mut failure) => {
            failure.hint = Some(hint.to_owned());
            failure.into()
        }
        Err(error) => error,
    }
}

command! {
    pub RUN_CREATE = ["airflow", "run", "create"], Write,
    "Trigger a DAG run, with a conf and optionally a logical date",
    keywords: ["trigger", "start", "kick", "off", "conf", "manual", "launch", "execute"],
    example: "airflow run create etl_nightly --conf '{\"day\":\"2026-09-28\"}'",
    run: run_create,
}

// ---------- airflow run wait ----------

#[derive(Debug, Serialize, JsonSchema)]
pub struct Waited {
    id: String,
    state: Option<String>,
    /// It reached success or failed.
    done: bool,
    waited_s: u64,
    duration: Option<i64>,
    failed: Vec<String>,
}

fn run_wait(ctx: &Ctx, args: RunIdArgs) -> Result<Waited> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = args.locate(&airflow, ctx)?;
    let started = Instant::now();
    let mut pause = FIRST_POLL;
    loop {
        let run = client.get(&id.run_path())?;
        let state = text(&run["state"]);
        let mut waited = Waited {
            id: id.run_id(),
            state: state.clone(),
            done: matches!(state.as_deref(), Some("success" | "failed")),
            waited_s: started.elapsed().as_secs(),
            duration: seconds(&run["start_date"], &run["end_date"]),
            failed: Vec::new(),
        };
        match state.as_deref() {
            Some("success") => return Ok(waited),
            Some("failed") => {
                let (tasks, _) = client.list(
                    &format!("{}/taskInstances", id.run_path()),
                    "state=failed",
                    "task_instances",
                    100,
                )?;
                waited.failed = failed_ids(&tasks);
                let hint = match waited.failed.first() {
                    Some(first) => format!("agent-cli airflow task logs {first} --tail 200"),
                    None => format!(
                        "agent-cli airflow run get {} --fields tasks,note",
                        id.run_id()
                    ),
                };
                return Err(
                    Failure::new(Exit::Failed, format!("run {} failed", id.run_id()))
                        .hint(hint)
                        .with_data(waited)
                        .into(),
                );
            }
            _ => {}
        }
        let left = ctx.deadline().saturating_duration_since(Instant::now());
        if left <= MARGIN {
            let paused = state.as_deref() == Some("queued")
                && client.get(&id.dag_path())?["is_paused"].as_bool() == Some(true);
            let hint = if paused {
                format!(
                    "the DAG is paused, so the run stays queued: agent-cli airflow dag update {} --paused false",
                    id.dag
                )
            } else {
                format!(
                    "the run keeps going; run the same command again: agent-cli airflow run wait {}",
                    id.run_id()
                )
            };
            return Err(Failure::timed_out(format!(
                "run {} is still {} after {}s",
                id.run_id(),
                state.as_deref().unwrap_or("going"),
                started.elapsed().as_secs()
            ))
            .hint(hint)
            .with_data(waited)
            .into());
        }
        std::thread::sleep(pause.min(left - MARGIN));
        pause = pause.mul_f64(1.5).min(LONGEST_POLL);
    }
}

command! {
    pub RUN_WAIT = ["airflow", "run", "wait"], Read,
    "Wait for a DAG run: exit 0 if it succeeded, 1 if it failed, 124 if still going",
    keywords: ["poll", "finish", "until", "block", "complete", "dag", "dagrun", "watch"],
    example: "airflow run wait etl_nightly/latest",
    timeout: 100,
    run: run_wait,
}

// ---------- airflow run retry ----------

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
    let path = format!("{}/clear", id.run_path());
    // The server's own dry run says what a clear would reset; a real clear
    // must send dry_run false, since true is the default.
    let preview = client.preview(&path, json!({"dry_run": true, "only_failed": true}))?;
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
    client.change(
        Effect::Write,
        Method::Post,
        &path,
        json!({"dry_run": false, "only_failed": true}),
    )?;
    ctx.note(format!(
        "[next: agent-cli airflow run wait {}]",
        id.run_id()
    ));
    Ok(Retried {
        id: id.run_id(),
        cleared,
    })
}

/// The task instances a clear's dry run listed, without a try: the next try
/// is the one that will run.
pub(crate) fn cleared_ids(preview: &Value) -> Vec<String> {
    preview["task_instances"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|task| ti_id(task, false))
        .collect()
}

command! {
    pub RUN_RETRY = ["airflow", "run", "retry"], Write,
    "Retry the failed and upstream_failed tasks of a DAG run",
    keywords: ["clear", "rerun", "failed", "again", "resume", "restart"],
    example: "airflow run retry etl_nightly/latest",
    run: run_retry,
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::{Duration, Instant};

    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testkit::{airflow, dry_run, paths};

    pub(crate) const RUN: &str = "scheduled__2026-09-29T00:00:00+00:00";
    pub(crate) const RUN_PATH: &str =
        "dags/etl_nightly/dagRuns/scheduled__2026-09-29T00%3A00%3A00%2B00%3A00";

    pub(crate) fn run(state: &str) -> Value {
        json!({"dag_run_id": RUN, "dag_id": "etl_nightly", "state": state, "run_type": "scheduled",
            "logical_date": "2026-09-29T00:00:00+00:00", "run_after": "2026-09-29T00:00:00+00:00",
            "start_date": "2026-09-29T00:00:05.12+00:00", "end_date": "2026-09-29T00:43:58.9+00:00",
            "triggered_by": "timetable", "conf": {"day": "2026-09-28", "api_token": "s3cr3t-conf-value"},
            "note": null, "dag_versions": [{"version_number": 6}, {"version_number": 7}]})
    }

    pub(crate) fn ti(task: &str, state: &str, attempt: i64) -> Value {
        json!({"id": "0199", "task_id": task, "dag_id": "etl_nightly", "dag_run_id": RUN,
            "map_index": -1, "state": state, "try_number": attempt, "max_tries": 1,
            "start_date": "2026-09-29T00:41:07+00:00", "end_date": "2026-09-29T00:43:55+00:00",
            "duration": 168.4, "operator": "PythonOperator", "executor": null, "queue": "default",
            "pool": "default_pool", "hostname": "etl-nightly-load-orders-q8x1k2vz", "note": null,
            "rendered_fields": {"op_kwargs": {"day": "2026-09-28"}}})
    }

    fn tasks(items: Vec<Value>) -> Answer {
        Answer::json(&json!({"total_entries": items.len(), "task_instances": items}))
    }

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
    fn run_get_counts_tasks_by_state_and_names_the_failed_ones_with_their_try() {
        let (outcome, transport) = airflow(
            &["airflow", "run", "get", "etl_nightly/latest"],
            vec![
                Answer::json(&json!({"dag_runs": [run("failed")], "total_entries": 1})),
                Answer::json(&run("failed")),
                tasks(vec![
                    ti("extract_orders", "success", 1),
                    ti("load_orders", "failed", 2),
                    ti("publish_report", "upstream_failed", 0),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], format!("etl_nightly/{RUN}"));
        assert_eq!(
            got["tasks"],
            json!({"failed": 1, "success": 1, "upstream_failed": 1})
        );
        assert_eq!(
            got["failed"],
            json!([format!("etl_nightly/{RUN}/load_orders/2")])
        );
        assert_eq!(got["dag_version"], 7);
        assert_eq!(got["conf"]["day"], "2026-09-28");
        assert_ne!(
            got["conf"]["api_token"], "s3cr3t-conf-value",
            "conf is redacted"
        );
        assert_eq!(
            paths(&transport),
            [
                "dags/etl_nightly/dagRuns?order_by=-run_after&limit=1".to_owned(),
                RUN_PATH.to_owned(),
                format!("{RUN_PATH}/taskInstances?limit=100&offset=0"),
            ]
        );
    }

    #[test]
    fn a_bare_run_id_takes_the_dag_from_the_flag_and_a_disagreeing_flag_is_exit_2() {
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "run",
                "get",
                RUN,
                "--dag",
                "etl_nightly",
                "--fields",
                "id",
            ],
            vec![Answer::json(&run("success")), tasks(vec![])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(paths(&transport)[0], RUN_PATH);
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "run",
                "get",
                &format!("etl_nightly/{RUN}"),
                "--dag",
                "orders_export",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("--dag says orders_export"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().is_empty());
        let url = "https://airflow.contoso.example/dags/etl_nightly/runs/scheduled__2026-09-29T00%3A00%3A00%2B00%3A00/details";
        let (outcome, transport) = airflow(
            &["airflow", "run", "get", url, "--fields", "id"],
            vec![Answer::json(&run("success")), tasks(vec![])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(paths(&transport)[0], RUN_PATH, "a UI URL names the run");
    }

    #[test]
    fn run_create_sends_a_null_logical_date_and_notes_a_paused_dag() {
        let paused = crate::dag::tests::dag("etl_nightly", true);
        let (plans, stderr) = dry_run(
            &[
                "airflow",
                "run",
                "create",
                "etl_nightly",
                "--conf",
                "{\"day\":\"2026-09-28\"}",
                "--note",
                "rerun for finance",
            ],
            vec![Answer::json(&paused)],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            "https://airflow.contoso.example/api/v2/dags/etl_nightly/dagRuns"
        );
        assert_eq!(
            plans[0]["body"],
            json!({"logical_date": null, "conf": {"day": "2026-09-28"}, "note": "rerun for finance"})
        );
        assert!(
            stderr.contains("[DAG etl_nightly is paused: the run stays queued until agent-cli airflow dag update etl_nightly --paused false]"),
            "{stderr}"
        );

        let active = crate::dag::tests::dag("etl_nightly", false);
        let mut created = run("queued");
        created["dag_run_id"] = json!("manual__2026-09-29T12:00:00+00:00");
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "run",
                "create",
                "etl_nightly",
                "--logical-date",
                "now",
            ],
            vec![Answer::json(&active), Answer::json(&created)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["id"],
            "etl_nightly/manual__2026-09-29T12:00:00+00:00"
        );
        let body = transport.sent()[1].body.clone().unwrap();
        assert!(
            body["logical_date"].as_str().unwrap().ends_with('Z'),
            "{body}"
        );
        assert!(
            outcome.stderr.contains(
                "[next: agent-cli airflow run wait etl_nightly/manual__2026-09-29T12:00:00+00:00]"
            ),
            "{}",
            outcome.stderr
        );

        for (status, body, code, hint) in [
            (
                409,
                r#"{"detail":"A DAG Run already exists"}"#,
                5,
                "agent-cli airflow run list --dag etl_nightly",
            ),
            (
                400,
                r#"{"detail":"DAG with dag_id: 'etl_nightly' has import errors and cannot be triggered"}"#,
                2,
                "agent-cli airflow import-error list",
            ),
        ] {
            let (outcome, _) = airflow(
                &["airflow", "run", "create", "etl_nightly"],
                vec![Answer::json(&active), Answer::status(status, body)],
            );
            assert_eq!(outcome.code, code, "{outcome:?}");
            assert!(
                outcome.stderr.contains(&format!("hint: {hint}")),
                "{}",
                outcome.stderr
            );
        }
        let (outcome, transport) = airflow(
            &["airflow", "run", "create", "etl_nightly", "--conf", "[1]"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn wait_exits_0_on_success_and_1_with_the_failed_tasks() {
        let (outcome, _) = airflow(
            &["airflow", "run", "wait", &format!("etl_nightly/{RUN}")],
            vec![Answer::json(&run("success"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["done"], true);

        let (outcome, transport) = airflow(
            &["airflow", "run", "wait", &format!("etl_nightly/{RUN}")],
            vec![
                Answer::json(&run("failed")),
                tasks(vec![ti("load_orders", "failed", 2)]),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(
            outcome.json()["failed"],
            json!([format!("etl_nightly/{RUN}/load_orders/2")])
        );
        assert!(
            outcome.stderr.contains(&format!(
                "hint: agent-cli airflow task logs etl_nightly/{RUN}/load_orders/2 --tail 200"
            )),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            paths(&transport)[1],
            format!("{RUN_PATH}/taskInstances?state=failed&limit=100&offset=0")
        );
    }

    #[test]
    fn wait_at_the_deadline_is_124_and_a_paused_dag_gets_the_unpause_hint() {
        let started = Instant::now();
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "run",
                "wait",
                &format!("etl_nightly/{RUN}"),
                "--timeout",
                "3",
            ],
            vec![
                Answer::json(&run("queued")),
                Answer::json(&run("queued")),
                Answer::json(&crate::dag::tests::dag("etl_nightly", true)),
            ],
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert_eq!(outcome.json()["state"], "queued", "its current state");
        assert_eq!(transport.remaining(), 0);
        assert!(
            outcome.stderr.contains("hint: the DAG is paused, so the run stays queued: agent-cli airflow dag update etl_nightly --paused false"),
            "{}",
            outcome.stderr
        );
    }

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
}
