//! `airflow xcom`: what a task instance handed its downstream tasks. An
//! XCom's id is `DAG/RUN/TASK[:MAP]@KEY`: the task instance's id, then its key.

pub(crate) mod get;
pub(crate) mod list;

use crate::client::{Ref, segment};

/// `…/taskInstances/TASK/xcomEntries`, every map index's.
fn entries_path(id: &Ref) -> String {
    format!(
        "{}/taskInstances/{}/xcomEntries",
        id.run_path(),
        segment(&id.task)
    )
}

/// `DAG/RUN/TASK[:MAP]`: what xcom list takes, and an XCom's id before `@KEY`.
fn task_id(id: &Ref, map: Option<i64>) -> String {
    match map {
        Some(map) => format!("{}/{}:{map}", id.run_id(), id.task),
        None => format!("{}/{}", id.run_id(), id.task),
    }
}

/// `map_index=N` for a mapped task instance's id, else nothing (every index).
fn map_query(id: &Ref) -> String {
    id.map
        .map(|map| format!("map_index={map}"))
        .unwrap_or_default()
}
