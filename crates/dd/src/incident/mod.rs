//! `dd incident`: Datadog incidents, found by number, UUID or URL.

pub(crate) mod get;
pub(crate) mod list;

use agent_cli_core::{Ctx, utc};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Dd, text};

#[derive(Debug, Serialize, JsonSchema)]
pub struct IncidentRow {
    /// What `dd incident get` takes: the number Datadog shows.
    id: i64,
    title: String,
    state: Option<String>,
    severity: Option<String>,
    created: Option<String>,
    resolved: Option<String>,
}

fn incident_row(incident: &Value) -> IncidentRow {
    let attributes = &incident["attributes"];
    IncidentRow {
        id: attributes["public_id"].as_i64().unwrap_or_default(),
        title: text(&attributes["title"]).unwrap_or_default(),
        state: text(&attributes["state"]),
        severity: text(&attributes["severity"]),
        created: text(&attributes["created"]).map(|at| utc(&at)),
        resolved: text(&attributes["resolved"]).map(|at| utc(&at)),
    }
}

/// The incidents search: every hit is wrapped in its own `data`.
fn search_incidents(dd: &Dd, ctx: &Ctx, query: String, size: usize) -> Result<Vec<Value>> {
    let found = dd.get(
        ctx,
        "/api/v2/incidents/search",
        &[
            ("query", query),
            ("sort", "-created".to_owned()),
            ("page[size]", size.to_string()),
        ],
    )?;
    Ok(found["data"]["attributes"]["incidents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|hit| hit["data"].clone())
        .collect())
}
