//! `airflow dag`: DAGs (their status, schedule and params, pausing).

pub(crate) mod diff;
pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod update;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use agent_cli_core::status_of;
use anyhow::Result;

use crate::client::{Client, Ref, stamp, text};

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| text(item).or_else(|| text(&item["name"])))
        .collect()
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
    /// True when the DAG's file is gone from its bundle: Airflow keeps the
    /// DAG and its runs, and starts none.
    stale: Option<bool>,
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
    /// True when the DAG's file is gone: Airflow starts no runs of it.
    stale: Option<bool>,
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

/// A DAG's details as `dag get` prints them (and `dag diff` compares),
/// with the runs it was given.
fn detail(dag: &Value, recent_runs: Vec<RecentRun>) -> DagDetail {
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
    let row = dag_row(dag);
    DagDetail {
        id: row.id,
        paused: row.paused,
        schedule: row.schedule,
        schedule_text: text(&dag["timetable_description"]),
        next_run: row.next_run,
        next_logical_date: stamp(&dag["next_dagrun_logical_date"])
            .or_else(|| stamp(&dag["next_dagrun"])),
        catchup: dag["catchup"].as_bool().unwrap_or_default(),
        max_active_runs: dag["max_active_runs"].as_i64(),
        owners: row.owners,
        tags: row.tags,
        file: row.file,
        bundle: text(&dag["bundle_name"]),
        version: dag["latest_dag_version"]["version_number"].as_i64(),
        last_parsed: stamp(&dag["last_parsed_time"]),
        import_errors: row.import_errors,
        stale: row.stale,
        description: text(&dag["description"]),
        params,
        recent_runs,
    }
}

fn dag_row(dag: &Value) -> DagRow {
    DagRow {
        id: dag["dag_id"].as_str().unwrap_or_default().to_owned(),
        paused: dag["is_paused"].as_bool().unwrap_or_default(),
        // Airflow 2 names the schedule `schedule_interval` and the next
        // run's due time `next_dagrun_create_after`.
        schedule: text(&dag["timetable_summary"])
            .or_else(|| text(&dag["schedule_interval"]["value"])),
        next_run: next_run(dag),
        tags: strings(&dag["tags"]),
        owners: strings(&dag["owners"]),
        file: crate::source::dag_file(dag),
        import_errors: dag["has_import_errors"].as_bool().unwrap_or_default(),
        // Airflow 2 says the opposite: `is_active` false.
        stale: dag["is_stale"]
            .as_bool()
            .or_else(|| dag["is_active"].as_bool().map(|active| !active))
            .filter(|stale| *stale),
    }
}

/// A DAG's details. Airflow 2 reads them from the parsed DAG, so a stale
/// one (its file gone) is a 404 there: its own record says it is stale.
fn details(client: &Client, id: &Ref) -> Result<Value> {
    match client.get(&format!("{}/details", id.dag_path())) {
        Err(error) if status_of(&error) == Some(404) && client.v1()? => {
            client.get(&id.dag_path()).map_err(|_| error)
        }
        other => other,
    }
}

/// When the next scheduled run is due.
fn next_run(dag: &Value) -> Option<String> {
    stamp(&dag["next_dagrun_run_after"]).or_else(|| stamp(&dag["next_dagrun_create_after"]))
}
