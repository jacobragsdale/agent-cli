//! What the dag, run and task commands share about DAG runs: the states a
//! run can be in, and the task instances a clear resets.

use serde_json::Value;

use crate::client::ti_id;

/// A run's states, as Airflow names them.
pub(crate) const RUN_STATES: [&str; 4] = ["queued", "running", "success", "failed"];

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
