//! `airflow dag`: DAGs (their status, schedule and params, pausing).

pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod update;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{stamp, text};

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
