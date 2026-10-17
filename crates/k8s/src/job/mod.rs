//! `k8s job …`: run-to-completion pods, read from kubectl's JSON, with the
//! CronJob that made each.

pub(crate) mod delete;
pub(crate) mod diff;
pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::kubectl::{Target, non_empty};

#[derive(Debug, Serialize, JsonSchema)]
pub struct JobRow {
    /// `cluster/namespace/name`: what `job get` takes.
    pub(crate) id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// Running, Complete, Failed or Suspended.
    pub(crate) status: String,
    /// Pods succeeded of completions wanted: 1/1.
    completions: String,
    /// Pods that failed, retries included.
    failed: i64,
    started: Option<String>,
    /// Seconds from start to finish, or to now while it runs.
    duration: Option<i64>,
    /// The CronJob's id that made it: what `cronjob get` takes.
    cronjob: Option<String>,
}

pub(crate) fn row(target: &Target, item: &Value) -> Option<JobRow> {
    let status = &item["status"];
    let stamp = |value: &Value| OffsetDateTime::parse(value.as_str()?, &Rfc3339).ok();
    let word = status_word(item);
    let started = stamp(&status["startTime"]);
    // A failed Job has no completionTime: it ended when it was marked Failed.
    let ended = stamp(&status["completionTime"])
        .or_else(|| finished(item, "Failed").and_then(|held| stamp(&held["lastTransitionTime"])))
        .or_else(|| (word == "Running").then(agent_cli_core::now));
    let cronjob = item["metadata"]["ownerReferences"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|owner| owner["kind"] == "CronJob")
        .and_then(|owner| non_empty(&owner["name"]))
        .map(|owner| {
            let namespace = item["metadata"]["namespace"].as_str().unwrap_or_default();
            format!("{}/{namespace}/{owner}", target.scope)
        });
    Some(JobRow {
        id: target.id(item),
        name: item["metadata"]["name"].as_str()?.to_owned(),
        namespace: target.row_namespace(item),
        status: word,
        completions: format!(
            "{}/{}",
            status["succeeded"].as_i64().unwrap_or(0),
            item["spec"]["completions"].as_i64().unwrap_or(1)
        ),
        failed: status["failed"].as_i64().unwrap_or(0),
        started: started.map(agent_cli_core::utc_time),
        duration: started
            .zip(ended)
            .map(|(started, ended)| (ended - started).whole_seconds()),
        cronjob,
    })
}

/// kubectl's STATUS for a Job: its Complete or Failed condition, else
/// Suspended or Running.
fn status_word(item: &Value) -> String {
    let word = if finished(item, "Complete").is_some() {
        "Complete"
    } else if finished(item, "Failed").is_some() {
        "Failed"
    } else if item["spec"]["suspend"] == true {
        "Suspended"
    } else {
        "Running"
    };
    word.to_owned()
}

/// The condition of `kind` (Complete, Failed) when it holds.
fn finished<'a>(item: &'a Value, kind: &str) -> Option<&'a Value> {
    item["status"]["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|condition| condition["type"] == kind && condition["status"] == "True")
}
