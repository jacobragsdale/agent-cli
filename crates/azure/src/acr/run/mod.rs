//! What `acr run list`, `get` and `logs` share: ACR Tasks runs, read through
//! ARM's `registries/{name}/runs` (an `az acr build` or a task's trigger).
//! A run's id is `registry/runid` (`contosoacr/ca1f`); its output images are
//! acr image ids, which `acr manifest get` takes.

pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod logs;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::acr::{arm_id, named};
use crate::client::{parse_stamp, stamp, text};
use crate::config::Azure;
use crate::graph::Registry;
use crate::net::list;

/// Runs have only ever had preview api-versions; this is the one `az acr`
/// sends.
pub(crate) const RUNS_API: &str = "2019-06-01-preview";

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunRow {
    /// `registry/runid`: what `acr run get` and `acr run logs` take.
    id: String,
    registry: String,
    run: String,
    /// The task that ran, absent for an `az acr build`.
    task: Option<String>,
    /// Queued, Started, Running, Succeeded, Failed, Canceled, Error or Timeout.
    status: Option<String>,
    /// What started it: `commit 3f9e2a1 on main`, `base image update`,
    /// `timer NAME` or `manual`.
    trigger: String,
    /// The images it pushed, as `acr manifest get` takes them.
    images: Vec<String>,
    /// RFC 3339.
    created: Option<String>,
    started: Option<String>,
    finished: Option<String>,
    /// Seconds from start to finish.
    duration: Option<i64>,
    error: Option<String>,
}

impl RunRow {
    pub(crate) fn new(registry: &Registry, run: &Value) -> Option<Self> {
        let props = &run["properties"];
        let id = text(&props["runId"]).or_else(|| text(&run["name"]))?;
        let at = |key: &str| text(&props[key]).as_deref().and_then(parse_stamp);
        Some(Self {
            id: format!("{}/{id}", registry.name),
            registry: registry.name.clone(),
            run: id,
            task: text(&props["task"]),
            status: text(&props["status"]),
            trigger: trigger(props),
            images: list(&props["outputImages"])
                .iter()
                .filter_map(|image| {
                    let reference = text(&image["tag"])
                        .map(|tag| format!(":{tag}"))
                        .or_else(|| text(&image["digest"]).map(|digest| format!("@{digest}")))?;
                    Some(format!(
                        "{}/{}{reference}",
                        text(&image["registry"]).unwrap_or_else(|| registry.login_server.clone()),
                        text(&image["repository"])?
                    ))
                })
                .collect(),
            created: stamp(&props["createTime"]),
            started: stamp(&props["startTime"]),
            finished: stamp(&props["finishTime"]),
            duration: at("startTime")
                .zip(at("finishTime"))
                .map(|(start, finish)| (finish - start).whole_seconds()),
            error: text(&props["runErrorMessage"]),
        })
    }

    /// Whether it ended without what it set out to make.
    pub(crate) fn failed(&self) -> bool {
        matches!(self.status.as_deref(), Some("Failed" | "Error" | "Timeout"))
    }
}

fn trigger(props: &Value) -> String {
    let source = &props["sourceTrigger"];
    if source.is_object() {
        let event = text(&source["eventType"]).unwrap_or_else(|| "commit".to_owned());
        let commit = text(&source["commitId"])
            .or_else(|| text(&source["pullRequestId"]))
            .map(|held| held.chars().take(7).collect::<String>());
        let branch = text(&source["branchName"]).map(|branch| format!(" on {branch}"));
        return format!(
            "{} {}{}",
            event.to_ascii_lowercase(),
            commit.unwrap_or_default(),
            branch.unwrap_or_default()
        );
    }
    if props["imageUpdateTrigger"].is_object() {
        return "base image update".to_owned();
    }
    if let Some(name) = text(&props["timerTrigger"]["timerTriggerName"]) {
        return format!("timer {name}");
    }
    match text(&props["runType"]).as_deref() {
        Some("QuickRun") | None => "manual".to_owned(),
        Some(other) => other.to_ascii_lowercase(),
    }
}

/// The run `raw` names and its registry: `registry/runid` (a name or login
/// server), the run's ARM id, or a bare run id in `--registry` or the only
/// registry in reach.
pub(crate) fn locate(
    ctx: &Ctx,
    azure: &Azure,
    raw: &str,
    flag: Option<&str>,
) -> Result<(Registry, String)> {
    let raw = raw.trim().trim_end_matches('/');
    let segments: Vec<&str> = raw.split('/').collect();
    let after = |word: &str| {
        segments
            .iter()
            .position(|held| held.eq_ignore_ascii_case(word))
            .and_then(|at| segments.get(at + 1).copied())
    };
    let (registry, run) = match (after("registries"), after("runs"), segments.as_slice()) {
        (Some(registry), Some(run), _) => (Some(registry), run),
        (_, _, [registry, run]) => (Some(*registry), *run),
        (_, _, [run]) => (None, *run),
        _ => {
            return Err(Failure::usage(format!(
                "{raw:?} is not a run id: REGISTRY/RUN as acr run list prints it, or its ARM id"
            ))
            .hint("agent-cli acr run list --fields id,status")
            .into());
        }
    };
    let found = match registry.or(flag) {
        Some(held) => named(ctx, azure, held)?,
        None => only(ctx, azure)?,
    };
    if let (Some(_), Some(flag)) = (registry, flag)
        && !found.name.eq_ignore_ascii_case(flag)
    {
        return Err(Failure::usage(format!(
            "{raw} is in registry {}, and --registry says {flag}",
            found.name
        ))
        .into());
    }
    Ok((found, run.to_owned()))
}

/// The one registry in reach, for a run named without one.
fn only(ctx: &Ctx, azure: &Azure) -> Result<Registry> {
    let mut found = crate::acr::registries(ctx, azure, &[])?;
    if found.len() == 1 {
        return Ok(found.remove(0));
    }
    let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
    Err(Failure::usage(format!(
        "a run id is per registry; name one of {} as REGISTRY/RUN or with --registry",
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(", ")
        }
    ))
    .into())
}

/// The ARM path of the registry's runs, which `below` (`/ca1f`) extends.
pub(crate) fn runs(registry: &Registry, below: &str) -> String {
    format!("{}/runs{below}", arm_id(registry))
}
