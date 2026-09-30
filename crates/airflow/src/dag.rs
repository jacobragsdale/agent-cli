//! DAGs (their status, schedule and params, pausing) and the import errors
//! that keep a DAG file from loading.

use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Airflow, At, Want, note_more, query_value, seconds, stamp, text};

/// A run's states, as Airflow names them.
pub(crate) const RUN_STATES: [&str; 4] = ["queued", "running", "success", "failed"];

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| text(item).or_else(|| text(&item["name"])))
        .collect()
}

// ---------- airflow dag list ----------

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

#[derive(Debug, Serialize, JsonSchema)]
pub struct DagRow {
    /// The dag_id: what dag get, run list --dag and run create take.
    id: String,
    paused: bool,
    /// The timetable, e.g. a cron expression.
    schedule: Option<String>,
    /// When the next scheduled run is due (RFC 3339).
    next_run: Option<String>,
    tags: Vec<String>,
    owners: Vec<String>,
    /// The DAG file, relative to its bundle.
    file: Option<String>,
    /// The file failed to parse: see import-error list.
    import_errors: bool,
}

fn dag_row(dag: &Value) -> DagRow {
    DagRow {
        id: dag["dag_id"].as_str().unwrap_or_default().to_owned(),
        paused: dag["is_paused"].as_bool().unwrap_or_default(),
        schedule: text(&dag["timetable_summary"]),
        next_run: stamp(&dag["next_dagrun_run_after"]),
        tags: strings(&dag["tags"]),
        owners: strings(&dag["owners"]),
        file: text(&dag["relative_fileloc"]).or_else(|| text(&dag["fileloc"])),
        import_errors: dag["has_import_errors"].as_bool().unwrap_or_default(),
    }
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

// ---------- airflow dag get ----------

#[derive(clap::Args)]
pub struct DagGetArgs {
    /// The DAG: its id, or its Airflow UI URL
    dag: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DagDetail {
    id: String,
    paused: bool,
    schedule: Option<String>,
    /// The schedule in words.
    schedule_text: Option<String>,
    next_run: Option<String>,
    next_logical_date: Option<String>,
    /// Unpausing runs every missed interval.
    catchup: bool,
    max_active_runs: Option<i64>,
    owners: Vec<String>,
    tags: Vec<String>,
    file: Option<String>,
    bundle: Option<String>,
    /// The DAG version number.
    version: Option<i64>,
    last_parsed: Option<String>,
    import_errors: bool,
    description: Option<String>,
    /// What run create --conf may set.
    params: Vec<Param>,
    /// The last five runs, newest first.
    recent_runs: Vec<RecentRun>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Param {
    name: String,
    default: Value,
    description: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RecentRun {
    /// DAG/RUN: what run get takes.
    id: String,
    state: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    run_after: Option<String>,
    /// Seconds.
    duration: Option<i64>,
}

fn dag_get(ctx: &Ctx, args: DagGetArgs) -> Result<DagDetail> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = airflow.locate(
        ctx,
        args.at.instance.as_deref(),
        &args.dag,
        Want::Dag,
        None,
        None,
    )?;
    let dag = client.get(&format!("{}/details", id.dag_path()))?;
    let runs = client.get(&format!(
        "{}/dagRuns?order_by=-run_after&limit=5",
        id.dag_path()
    ))?;
    let params = dag["params"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, param)| {
            let described = param.is_object() && param.get("value").is_some();
            Param {
                name: name.clone(),
                default: if described {
                    param["value"].clone()
                } else {
                    param.clone()
                },
                description: text(&param["description"]),
            }
        })
        .collect();
    let row = dag_row(&dag);
    Ok(DagDetail {
        id: row.id,
        paused: row.paused,
        schedule: row.schedule,
        schedule_text: text(&dag["timetable_description"]),
        next_run: row.next_run,
        next_logical_date: stamp(&dag["next_dagrun_logical_date"]),
        catchup: dag["catchup"].as_bool().unwrap_or_default(),
        max_active_runs: dag["max_active_runs"].as_i64(),
        owners: row.owners,
        tags: row.tags,
        file: row.file,
        bundle: text(&dag["bundle_name"]),
        version: dag["latest_dag_version"]["version_number"].as_i64(),
        last_parsed: stamp(&dag["last_parsed_time"]),
        import_errors: row.import_errors,
        description: text(&dag["description"]),
        params,
        recent_runs: runs["dag_runs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|run| RecentRun {
                id: format!(
                    "{}/{}",
                    run["dag_id"].as_str().unwrap_or(&id.dag),
                    run["dag_run_id"].as_str().unwrap_or_default()
                ),
                state: text(&run["state"]),
                kind: text(&run["run_type"]),
                run_after: stamp(&run["run_after"]),
                duration: seconds(&run["start_date"], &run["end_date"]),
            })
            .collect(),
    })
}

command! {
    pub DAG_GET = ["airflow", "dag", "get"], Read,
    "Show a DAG's status: paused, next run, params, schedule and its last five runs",
    keywords: ["status", "next", "params", "schedule", "recent", "when", "catchup", "details"],
    example: "airflow dag get etl_nightly --fields paused,next_run,recent_runs",
    run: dag_get,
}

// ---------- airflow dag update ----------

#[derive(clap::Args)]
pub struct DagUpdateArgs {
    /// The DAG: its id, or its Airflow UI URL
    dag: String,
    /// true pauses the DAG, false unpauses it
    #[arg(long, required = true, action = clap::ArgAction::Set)]
    paused: bool,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DagUpdated {
    id: String,
    paused: bool,
    next_run: Option<String>,
}

fn dag_update(ctx: &Ctx, args: DagUpdateArgs) -> Result<DagUpdated> {
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
    let dag = client.get(&format!("{}/details", id.dag_path()))?;
    if !args.paused && dag["catchup"].as_bool() == Some(true) {
        ctx.note(format!(
            "[catchup is on: unpausing {} schedules every interval missed since {}]",
            id.dag,
            stamp(&dag["next_dagrun_logical_date"]).unwrap_or_else(|| "it was paused".to_owned())
        ));
    }
    let updated = client.change(
        Effect::Write,
        Method::Patch,
        &format!("{}?update_mask=is_paused", id.dag_path()),
        json!({"is_paused": args.paused}),
    )?;
    Ok(DagUpdated {
        id: id.dag,
        paused: updated["is_paused"].as_bool().unwrap_or(args.paused),
        next_run: stamp(&updated["next_dagrun_run_after"]),
    })
}

command! {
    pub DAG_UPDATE = ["airflow", "dag", "update"], Write,
    "Pause or unpause a DAG",
    keywords: ["pause", "unpause", "enable", "disable", "resume", "stop", "schedule"],
    example: "airflow dag update etl_nightly --paused true",
    run: dag_update,
}

// ---------- airflow import-error list / get ----------

#[derive(clap::Args)]
pub struct ImportErrorListArgs {
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ImportErrorRow {
    /// What import-error get takes.
    id: i64,
    /// The DAG file that failed to parse.
    file: Option<String>,
    bundle: Option<String>,
    timestamp: Option<String>,
    /// The exception, the stack trace's last line.
    error: Option<String>,
}

fn import_error_row(error: &Value) -> ImportErrorRow {
    ImportErrorRow {
        id: error["import_error_id"].as_i64().unwrap_or_default(),
        file: text(&error["filename"]),
        bundle: text(&error["bundle_name"]),
        timestamp: stamp(&error["timestamp"]),
        error: error["stack_trace"]
            .as_str()
            .and_then(|trace| {
                trace
                    .lines()
                    .rev()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
            })
            .map(str::to_owned),
    }
}

fn import_error_list(ctx: &Ctx, args: ImportErrorListArgs) -> Result<Vec<ImportErrorRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    let (errors, total) = client.list(
        "importErrors",
        "order_by=-timestamp",
        "import_errors",
        args.limit,
    )?;
    note_more(ctx, errors.len(), total);
    Ok(errors.iter().map(import_error_row).collect())
}

command! {
    pub IMPORT_ERROR_LIST = ["airflow", "import-error", "list"], Read,
    "List DAG files that fail to import, newest first: why a DAG is missing",
    keywords: ["broken", "parse", "syntax", "missing", "dag", "file", "exception", "load"],
    example: "airflow import-error list --fields id,file,error",
    run: import_error_list,
}

#[derive(clap::Args)]
pub struct ImportErrorGetArgs {
    /// The import error's id, from import-error list
    id: i64,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ImportErrorDetail {
    id: i64,
    file: Option<String>,
    bundle: Option<String>,
    timestamp: Option<String>,
    error: Option<String>,
    stack_trace: Option<String>,
}

fn import_error_get(ctx: &Ctx, args: ImportErrorGetArgs) -> Result<ImportErrorDetail> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    let error = client.get(&format!("importErrors/{}", args.id))?;
    let row = import_error_row(&error);
    Ok(ImportErrorDetail {
        id: row.id,
        file: row.file,
        bundle: row.bundle,
        timestamp: row.timestamp,
        error: row.error,
        stack_trace: text(&error["stack_trace"]),
    })
}

command! {
    pub IMPORT_ERROR_GET = ["airflow", "import-error", "get"], Read,
    "Show an import error's full stack trace",
    keywords: ["trace", "traceback", "full", "broken", "dag", "file"],
    example: "airflow import-error get 12",
    run: import_error_get,
}

#[cfg(test)]
pub(crate) mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testkit::{CONFIG, airflow, airflow_with, dry_run, paths};

    pub(crate) fn dag(id: &str, paused: bool) -> Value {
        json!({
            "dag_id": id, "dag_display_name": id, "is_paused": paused, "is_stale": false,
            "last_parsed_time": "2026-09-29T11:58:00.123456+00:00", "bundle_name": "dags-folder",
            "relative_fileloc": format!("{id}.py"), "fileloc": format!("/opt/airflow/dags/{id}.py"),
            "description": null, "timetable_summary": "0 0 * * *",
            "timetable_description": "At 00:00", "tags": [{"name": "etl", "dag_id": id}],
            "max_active_runs": 1, "has_import_errors": false,
            "next_dagrun_logical_date": "2026-09-30T00:00:00+00:00",
            "next_dagrun_run_after": "2026-09-30T00:00:00+00:00", "owners": ["data-eng"]
        })
    }

    fn run(id: &str, state: &str) -> Value {
        json!({"dag_run_id": id, "dag_id": "etl_nightly", "state": state, "run_type": "scheduled",
            "run_after": "2026-09-29T00:00:00+00:00", "start_date": "2026-09-29T00:00:05.1+00:00",
            "end_date": "2026-09-29T00:43:58.2+00:00"})
    }

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

    #[test]
    fn dag_get_joins_the_details_params_and_recent_runs() {
        let mut details = dag("etl_nightly", false);
        details["catchup"] = json!(false);
        details["params"] = json!({
            "day": {"__class": "airflow.sdk.definitions.param.Param", "value": null,
                "description": "The day to load", "schema": {"type": ["null", "string"]}},
            "full": {"__class": "airflow.sdk.definitions.param.Param", "value": false, "schema": {}}
        });
        details["latest_dag_version"] = json!({"version_number": 7});
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "dag",
                "get",
                "https://airflow.contoso.example/dags/etl_nightly/runs?x=1",
            ],
            vec![
                Answer::json(&details),
                Answer::json(
                    &json!({"dag_runs": [run("scheduled__2026-09-29T00:00:00+00:00", "failed")], "total_entries": 1}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["params"],
            json!([
                {"name": "day", "description": "The day to load"},
                {"name": "full", "default": false}
            ])
        );
        assert_eq!(got["version"], 7);
        assert_eq!(got["last_parsed"], "2026-09-29T11:58:00Z");
        assert_eq!(
            got["recent_runs"],
            json!([{"id": "etl_nightly/scheduled__2026-09-29T00:00:00+00:00", "state": "failed",
                "type": "scheduled", "run_after": "2026-09-29T00:00:00Z", "duration": 2633}])
        );
        assert_eq!(
            paths(&transport),
            [
                "dags/etl_nightly/details",
                "dags/etl_nightly/dagRuns?order_by=-run_after&limit=5"
            ]
        );
    }

    #[test]
    fn a_missing_dag_is_exit_4_hinting_the_list() {
        let (outcome, _) = airflow(
            &["airflow", "dag", "get", "nope"],
            vec![Answer::status(
                404,
                r#"{"detail":"The DAG with dag_id: `nope` was not found"}"#,
            )],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("was not found\nhint: agent-cli airflow dag list"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_url_on_an_unconfigured_host_is_exit_2_before_any_request() {
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "dag",
                "get",
                "https://other.contoso.example/dags/etl_nightly",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("configured: prod (https://airflow.contoso.example)"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn unpausing_patches_is_paused_and_notes_catchup() {
        let mut details = dag("etl_nightly", true);
        details["catchup"] = json!(true);
        let (plans, stderr) = dry_run(
            &[
                "airflow",
                "dag",
                "update",
                "etl_nightly",
                "--paused",
                "false",
            ],
            vec![Answer::json(&details)],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            "https://airflow.contoso.example/api/v2/dags/etl_nightly?update_mask=is_paused"
        );
        assert_eq!(plans[0]["body"], json!({"is_paused": false}));
        assert!(stderr.contains("[catchup is on: unpausing etl_nightly schedules every interval missed since 2026-09-30T00:00:00Z]"), "{stderr}");

        let (outcome, transport) = airflow(
            &[
                "airflow",
                "dag",
                "update",
                "etl_nightly",
                "--paused",
                "true",
            ],
            vec![
                Answer::json(&details),
                Answer::json(&dag("etl_nightly", true)),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "etl_nightly", "paused": true, "next_run": "2026-09-30T00:00:00Z"})
        );
        assert_eq!(transport.sent()[1].body, Some(json!({"is_paused": true})));
    }

    #[test]
    fn a_read_only_instance_refuses_a_change_even_as_a_dry_run() {
        let config = format!("{CONFIG}read_only = true\n");
        for extra in [None, Some("--dry-run")] {
            let mut argv = vec![
                "airflow",
                "dag",
                "update",
                "etl_nightly",
                "--paused",
                "true",
            ];
            argv.extend(extra);
            let (outcome, transport) = airflow_with(&config, &argv, vec![]);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(
                outcome.stderr.contains("instance \"prod\" is read_only"),
                "{}",
                outcome.stderr
            );
            assert!(transport.sent().is_empty());
        }
    }

    #[test]
    fn import_errors_list_the_exception_and_get_the_whole_trace() {
        let trace = "Traceback (most recent call last):\n  File \"/opt/airflow/dags/customer_sync.py\", line 3, in <module>\n    import contoso_sdk\nModuleNotFoundError: No module named 'contoso_sdk'\n";
        let error = json!({"import_error_id": 12, "timestamp": "2026-09-29T11:58:01.5+00:00",
            "filename": "customer_sync.py", "bundle_name": "dags-folder", "stack_trace": trace});
        let (outcome, transport) = airflow(
            &["airflow", "import-error", "list"],
            vec![Answer::json(
                &json!({"import_errors": [error.clone()], "total_entries": 1}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": 12, "file": "customer_sync.py", "bundle": "dags-folder",
                "timestamp": "2026-09-29T11:58:01Z",
                "error": "ModuleNotFoundError: No module named 'contoso_sdk'"}])
        );
        assert_eq!(
            paths(&transport),
            ["importErrors?order_by=-timestamp&limit=50&offset=0"]
        );

        let (outcome, transport) = airflow(
            &["airflow", "import-error", "get", "12"],
            vec![Answer::json(&error)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome.json()["stack_trace"]
                .as_str()
                .unwrap()
                .starts_with("Traceback")
        );
        assert_eq!(paths(&transport), ["importErrors/12"]);
    }
}
