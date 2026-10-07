//! `aisearch indexer`: what its verbs share. The service's own words
//! mislead here: an indexer's `status: "running"` means healthy, not
//! executing, so it prints as `health: ok`; the run's own state is
//! `last_status`.

pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod run;
pub(crate) mod wait;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::aisearch::refs::{Want, agree, parse};
use crate::aisearch::{Role, Search, list, segment};
use crate::client::{stamp, text};

/// How many of a run's errors and warnings print, and of its history.
const ITEMS: usize = 20;
const HISTORY: usize = 10;

/// The indexer an agent named, and the service it named with it.
pub(crate) fn named(raw: &str, service: Option<&str>) -> Result<(Option<String>, String)> {
    let found = parse(raw, Want::Indexer)?;
    let service = agree(raw, found.service, service, "service")?;
    Ok((service, found.indexer.unwrap_or_default()))
}

/// `running` means set up and able to run, not executing.
pub(crate) fn health(status: &Value) -> Option<String> {
    text(&status["status"]).map(|held| {
        if held == "running" {
            "ok".to_owned()
        } else {
            held
        }
    })
}

/// A run that did not succeed, or succeeded with failed items. A status the
/// service did not document (`persistentFailure`, or anything new) counts as
/// a failure, never as fine.
pub(crate) fn failing(status: &Value) -> bool {
    let last = &status["lastResult"];
    let run = text(&last["status"]);
    status["status"] == "error"
        || last["itemsFailed"]
            .as_u64()
            .is_some_and(|failed| failed > 0)
        || run.is_some_and(|run| !matches!(run.as_str(), "success" | "inProgress" | "reset"))
}

/// The indexer's status: `GET /indexers/{name}/status`.
pub(crate) fn status(search: &Search<'_>, name: &str) -> Result<Value> {
    search.get(
        &format!("/indexers/{}/status", segment(name)),
        Role::Definitions,
    )
}

/// The indexer and its data source, which `detail` reads around the status.
pub(crate) fn definitions(search: &Search<'_>, name: &str) -> Result<(Value, Value)> {
    let indexer = search.get(&format!("/indexers/{}", segment(name)), Role::Definitions)?;
    // Its connection string is always null on a GET; type and container are
    // what is read.
    let source = match text(&indexer["dataSourceName"]) {
        Some(source) => search.get(
            &format!("/datasources/{}", segment(&source)),
            Role::Definitions,
        )?,
        None => Value::Null,
    };
    Ok((indexer, source))
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IndexerDetail {
    /// SERVICE/INDEXER: what indexer get, run and wait take.
    pub(crate) id: String,
    /// The target index's id, SERVICE/INDEX.
    index: Option<String>,
    source: Source,
    skillset: Option<String>,
    /// An ISO 8601 interval, such as PT1H.
    schedule: Option<String>,
    disabled: bool,
    /// ok, error or unknown: the indexer's own health, not whether it runs now.
    health: Option<String>,
    last: Option<Run>,
    /// The last run's failed items: the first 20.
    errors: Vec<ItemError>,
    warnings: Vec<ItemWarning>,
    /// The last 10 runs, newest first.
    history: Vec<Run>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Source {
    name: Option<String>,
    /// azuresql, cosmosdb, azureblob, adlsgen2, azuretable, mysql …
    #[serde(rename = "type")]
    kind: Option<String>,
    /// A table, a collection or a blob container.
    container: Option<String>,
    query: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Run {
    /// inProgress, success, transientFailure, persistentFailure or reset.
    pub(crate) status: Option<String>,
    pub(crate) start: Option<String>,
    pub(crate) end: Option<String>,
    pub(crate) processed: Option<u64>,
    pub(crate) failed: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ItemError {
    /// The document's key, as document get takes it after SERVICE/INDEX/.
    key: Option<String>,
    message: Option<String>,
    /// Where it failed: an extraction step or a skill.
    name: Option<String>,
    status: Option<u64>,
    details: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ItemWarning {
    key: Option<String>,
    message: Option<String>,
    name: Option<String>,
}

pub(crate) fn run(result: &Value) -> Option<Run> {
    result.is_object().then(|| Run {
        status: text(&result["status"]),
        start: stamp(&result["startTime"]),
        end: stamp(&result["endTime"]),
        processed: result["itemsProcessed"].as_u64(),
        failed: result["itemsFailed"].as_u64(),
        error: text(&result["errorMessage"]),
    })
}

/// The indexer's row from its definition, data source and status.
pub(crate) fn detail(
    ctx: &Ctx,
    search: &Search<'_>,
    indexer: &Value,
    source: &Value,
    status: &Value,
) -> IndexerDetail {
    let name = text(&indexer["name"]).unwrap_or_default();
    let last = &status["lastResult"];
    let errors = list(&last["errors"]);
    let warnings = list(&last["warnings"]);
    if errors.len() > ITEMS || warnings.len() > ITEMS {
        ctx.note(format!(
            "[the last run has {} errors and {} warnings; the first {ITEMS} of each print]",
            errors.len(),
            warnings.len()
        ));
    }
    IndexerDetail {
        id: format!("{}/{name}", search.name()),
        index: text(&indexer["targetIndexName"]).map(|index| format!("{}/{index}", search.name())),
        source: Source {
            name: text(&indexer["dataSourceName"]),
            kind: text(&source["type"]),
            container: text(&source["container"]["name"]),
            query: text(&source["container"]["query"]),
        },
        skillset: text(&indexer["skillsetName"]),
        schedule: text(&indexer["schedule"]["interval"]),
        disabled: indexer["disabled"].as_bool().unwrap_or(false),
        health: health(status),
        last: run(last),
        errors: errors
            .iter()
            .take(ITEMS)
            .map(|error| ItemError {
                key: text(&error["key"]),
                message: text(&error["errorMessage"]),
                name: text(&error["name"]),
                status: error["statusCode"].as_u64(),
                details: text(&error["details"]),
            })
            .collect(),
        warnings: warnings
            .iter()
            .take(ITEMS)
            .map(|warning| ItemWarning {
                key: text(&warning["key"]),
                message: text(&warning["message"]),
                name: text(&warning["name"]),
            })
            .collect(),
        history: list(&status["executionHistory"])
            .iter()
            .take(HISTORY)
            .filter_map(run)
            .collect(),
    }
}

/// When the last run failed items, the document to look at next: is it
/// missing, or there but stale? Only a key document get can take (letters,
/// digits, `-`, `_`, `=`), and only with a target index.
pub(crate) fn next_note(ctx: &Ctx, row: &IndexerDetail) {
    let failed = row.last.as_ref().and_then(|last| last.failed).unwrap_or(0);
    let key = row.errors.iter().find_map(|error| error.key.clone());
    if failed == 0 {
        return;
    }
    if let (Some(index), Some(key)) = (&row.index, key)
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_=".contains(&byte))
    {
        ctx.note(format!(
            "[next: agent-cli aisearch document get {index}/{key}]"
        ));
    }
}

/// The failure an indexer run that did not succeed ends a command with.
pub(crate) fn failed(row: &IndexerDetail) -> Failure {
    let status = row
        .last
        .as_ref()
        .and_then(|last| last.status.clone())
        .unwrap_or_else(|| "unknown".to_owned());
    Failure::new(
        agent_cli_core::Exit::Failed,
        format!("indexer {}'s last run ended {status}", row.id),
    )
    .hint(format!(
        "its errors are in the data; after fixing the cause, `agent-cli aisearch indexer run {}`",
        row.id
    ))
}
