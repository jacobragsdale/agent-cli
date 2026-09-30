//! Pipeline runs (builds) and their logs. Read live: ticket-tui listed only
//! pipelines with a local clone, swallowed timeline errors, and exited 2 or 3
//! from `runs wait` in a way that clashed with usage errors. Here a wait's
//! outcome is exit 0 (succeeded), 1 (did not) or 124 (still going at the
//! deadline), with the run on stdout each way.

pub(crate) mod cancel;
pub(crate) mod create;
pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod logs;
pub(crate) mod retry;
pub(crate) mod wait;

use agent_cli_core::{Ctx, Method};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Ado, Body, list, short_branch, stamp, text};

// ---------- runs ----------

/// One run as a list shows it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunRow {
    id: i64,
    pipeline: Option<String>,
    pipeline_id: Option<i64>,
    build_number: Option<String>,
    /// notStarted, inProgress, cancelling or completed.
    status: Option<String>,
    /// succeeded, partiallySucceeded, failed or canceled, once completed.
    result: Option<String>,
    branch: Option<String>,
    commit: Option<String>,
    requested_by: Option<String>,
    reason: Option<String>,
    queued: Option<String>,
    started: Option<String>,
    finished: Option<String>,
    url: Option<String>,
}

fn run_row(build: &Value) -> RunRow {
    RunRow {
        id: build["id"].as_i64().unwrap_or_default(),
        pipeline: text(&build["definition"]["name"]),
        pipeline_id: build["definition"]["id"].as_i64(),
        build_number: text(&build["buildNumber"]),
        status: text(&build["status"]),
        result: text(&build["result"]),
        branch: build["sourceBranch"].as_str().map(short_branch),
        commit: text(&build["sourceVersion"]),
        requested_by: text(&build["requestedFor"]["displayName"]),
        reason: text(&build["reason"]),
        queued: stamp(&build["queueTime"]),
        started: stamp(&build["startTime"]),
        finished: stamp(&build["finishTime"]),
        url: text(&build["_links"]["web"]["href"]),
    }
}

fn build_url(ado: &Ado, id: i64) -> String {
    ado.code(&format!("build/builds/{id}"), "")
}

#[derive(clap::Args)]
pub struct RunIdArgs {
    /// The run's id: 8812, #8812 or its web URL
    id: String,
}

/// A run's timeline records: stages, phases, jobs and tasks. A run still
/// queued has none yet, and Azure DevOps answers that with an empty body;
/// any other failure is the command's.
fn timeline(ctx: &Ctx, ado: &Ado, id: i64) -> Result<Vec<Value>> {
    let url = ado.code(&format!("build/builds/{id}/timeline"), "");
    let response = ado.send(ctx, Method::Get, &url, Body::None, None)?;
    if response.body.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(list(&response.json()?["records"]).to_vec())
}

fn is(record: &Value, kind: &str) -> bool {
    record["type"]
        .as_str()
        .is_some_and(|held| held.eq_ignore_ascii_case(kind))
}

/// The log a record names; `0` is Azure DevOps for none.
fn log_id(record: &Value) -> Option<i64> {
    record["log"]["id"].as_i64().filter(|id| *id > 0)
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, build, dry_run};

    #[test]
    fn cancel_is_destructive_and_retry_is_a_patch_with_retry_set() {
        let plans = dry_run(&["ado", "run", "cancel", "991"], vec![]);
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/build/builds/991?api-version=7.1")
        );
        assert_eq!(plans[0]["body"], json!({"status": "cancelling"}));
        let (outcome, transport) = ado(&["ado", "run", "cancel", "991"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());

        let plans = dry_run(&["ado", "run", "retry", "991"], vec![]);
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/build/builds/991?retry=true&api-version=7.1")
        );
        let (outcome, _) = ado(
            &["ado", "run", "retry", "991"],
            vec![Answer::json(&build(991, "inProgress", None))],
        );
        assert_eq!(outcome.json()["status"], "inProgress");
    }
}
