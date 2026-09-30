//! `dd downtime`: monitors muted for a bounded time.

pub(crate) mod cancel;
pub(crate) mod create;
pub(crate) mod list;

use agent_cli_core::utc;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{cut, strings, text};
use crate::monitor::MESSAGE_MAX;

#[derive(Debug, Serialize, JsonSchema)]
pub struct DowntimeRow {
    /// What `dd downtime cancel` takes.
    id: String,
    /// active, scheduled, ended, canceled.
    status: Option<String>,
    monitor: Option<i64>,
    /// Or every monitor with these tags.
    monitor_tags: Vec<String>,
    scope: Option<String>,
    start: Option<String>,
    /// Empty: until canceled.
    end: Option<String>,
    message: Option<String>,
}

fn downtime_row(item: &Value) -> DowntimeRow {
    let attributes = &item["attributes"];
    let schedule = &attributes["schedule"];
    let current = if schedule["current_downtime"].is_object() {
        &schedule["current_downtime"]
    } else {
        schedule
    };
    let identifier = &attributes["monitor_identifier"];
    DowntimeRow {
        id: text(&item["id"]).unwrap_or_default(),
        status: text(&attributes["status"]),
        monitor: identifier["monitor_id"].as_i64(),
        monitor_tags: strings(&identifier["monitor_tags"]),
        scope: text(&attributes["scope"]),
        start: text(&current["start"]).map(|at| utc(&at)),
        end: text(&current["end"]).map(|at| utc(&at)),
        message: text(&attributes["message"]).map(|message| cut(&message, MESSAGE_MAX)),
    }
}
