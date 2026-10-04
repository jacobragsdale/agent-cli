//! `airflow dag`: DAGs (their status, schedule and params, pausing).

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
