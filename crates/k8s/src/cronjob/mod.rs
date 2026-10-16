//! `k8s cronjob …`: scheduled jobs, read from kubectl's JSON, with the runs
//! each made.

pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod run;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{Target, non_empty};

#[derive(Debug, Serialize, JsonSchema)]
pub struct CronJobRow {
    /// `cluster/namespace/name`: what `cronjob get` takes.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// Cron syntax: `0 1 * * *`.
    schedule: String,
    /// The zone the schedule is read in; the controller's (UTC on AKS) when
    /// left out.
    time_zone: Option<String>,
    /// A suspended cronjob makes no runs.
    suspended: bool,
    last_schedule: Option<String>,
    last_success: Option<String>,
    /// The ids of the jobs running now.
    active: Vec<String>,
}

fn row(target: &Target, item: &Value) -> Option<CronJobRow> {
    let spec = &item["spec"];
    let status = &item["status"];
    let namespace = item["metadata"]["namespace"].as_str().unwrap_or_default();
    Some(CronJobRow {
        id: target.id(item),
        name: item["metadata"]["name"].as_str()?.to_owned(),
        namespace: target.row_namespace(item),
        schedule: spec["schedule"].as_str().unwrap_or_default().to_owned(),
        time_zone: non_empty(&spec["timeZone"]).map(str::to_owned),
        suspended: spec["suspend"] == true,
        last_schedule: non_empty(&status["lastScheduleTime"]).map(agent_cli_core::utc),
        last_success: non_empty(&status["lastSuccessfulTime"]).map(agent_cli_core::utc),
        active: status["active"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|job| non_empty(&job["name"]))
            .map(|job| format!("{}/{namespace}/{job}", target.scope))
            .collect(),
    })
}
