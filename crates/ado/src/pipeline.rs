//! Pipelines, their runs (builds), run logs and the approvals a run waits on.
//! Read live: ticket-tui listed only pipelines with a local clone, swallowed
//! timeline errors, and exited 2 or 3 from `runs wait` in a way that clashed
//! with usage errors. Here a wait's outcome is exit 0 (succeeded), 1 (did
//! not) or 124 (still going at the deadline), with the run on stdout each way.

use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Effect, Exit, Failure, Method, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::client::{
    Ado, Body, Kind, PREVIEW_API, full_ref, list, query_value, rate_limit_pause, short_branch,
    stamp, text,
};
use crate::workitem::{self, WorkItemRef};

/// How often `run wait` asks, which is how often ticket-tui's watcher did.
const POLL: Duration = Duration::from_secs(15);
/// What a wait leaves of the deadline for its last poll and its answer.
const MARGIN: Duration = Duration::from_secs(2);

// ---------- ado pipeline list ----------

#[derive(clap::Args)]
pub struct PipelineListArgs {
    /// Only names containing this
    pattern: Option<String>,
    /// Only pipelines that build this repository
    #[arg(long)]
    repo: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PipelineRow {
    id: i64,
    name: Option<String>,
    folder: Option<String>,
    /// enabled, paused or disabled.
    queue_status: Option<String>,
    last_run: Option<LastRun>,
    url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LastRun {
    id: i64,
    status: Option<String>,
    result: Option<String>,
    branch: Option<String>,
    finished: Option<String>,
}

fn pipeline_list(ctx: &Ctx, args: PipelineListArgs) -> Result<Vec<PipelineRow>> {
    let ado = Ado::load(ctx)?;
    let mut query = format!(
        "includeLatestBuilds=true&queryOrder=definitionNameAscending&$top={}",
        args.limit + 1
    );
    if let Some(pattern) = &args.pattern {
        query.push_str(&format!(
            "&name={}",
            query_value(&format!("*{}*", pattern.trim()))
        ));
    }
    if let Some(repo) = &args.repo {
        let repo = ado.repo(ctx, repo)?;
        query.push_str(&format!("&repositoryId={}&repositoryType=TfsGit", repo.id));
    }
    let answer = ado.get(ctx, &ado.code("build/definitions", &query))?;
    let mut rows: Vec<PipelineRow> = list(&answer["value"])
        .iter()
        .filter_map(|definition| {
            let latest = &definition["latestBuild"];
            Some(PipelineRow {
                id: definition["id"].as_i64()?,
                name: text(&definition["name"]),
                folder: text(&definition["path"]),
                queue_status: text(&definition["queueStatus"]),
                last_run: latest["id"].as_i64().map(|id| LastRun {
                    id,
                    status: text(&latest["status"]),
                    result: text(&latest["result"]),
                    branch: latest["sourceBranch"].as_str().map(short_branch),
                    finished: stamp(&latest["finishTime"]),
                }),
                url: text(&definition["_links"]["web"]["href"]),
            })
        })
        .collect();
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!(
            "[first {}; --limit N, or narrow with PATTERN or --repo]",
            args.limit
        ));
    }
    Ok(rows)
}

command! {
    pub PIPELINE_LIST = ["ado", "pipeline", "list"], Read,
    "List build pipelines with their last run",
    keywords: ["definitions", "ci", "workflows", "find"],
    example: "ado pipeline list --repo web --fields id,name,last_run.result",
    run: pipeline_list,
}

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

#[derive(Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "camelCase")]
enum RunStatus {
    InProgress,
    NotStarted,
    Cancelling,
    Completed,
    All,
}

#[derive(Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "camelCase")]
enum RunResult {
    Succeeded,
    PartiallySucceeded,
    Failed,
    Canceled,
}

/// Why a run was queued, as Azure DevOps names it.
#[derive(Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "camelCase")]
enum RunReason {
    Manual,
    #[value(name = "individualCI")]
    IndividualCi,
    #[value(name = "batchedCI")]
    BatchedCi,
    Schedule,
    PullRequest,
    BuildCompletion,
    ResourceTrigger,
}

#[derive(clap::Args)]
pub struct RunListArgs {
    /// Pipeline name or id
    #[arg(long)]
    pipeline: Option<String>,
    /// The branch or tag it built: main, v1.4.2, refs/heads/main, refs/tags/v1.4.2
    #[arg(long)]
    branch: Option<String>,
    /// Queued after this
    #[arg(long)]
    since: Option<When>,
    /// Queued before this
    #[arg(long)]
    until: Option<When>,
    /// Runs in this state
    #[arg(long, value_enum)]
    status: Option<RunStatus>,
    /// Finished runs with this result
    #[arg(long, value_enum)]
    result: Option<RunResult>,
    /// Who queued it: name, email or @me
    #[arg(long)]
    requested_by: Option<String>,
    /// Why it ran: a push (individualCI, batchedCI), a PR, a schedule, by hand …
    #[arg(long, value_enum, ignore_case = true)]
    reason: Option<RunReason>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn run_list(ctx: &Ctx, args: RunListArgs) -> Result<Vec<RunRow>> {
    let ado = Ado::load(ctx)?;
    let mut query = format!("queryOrder=queueTimeDescending&$top={}", args.limit + 1);
    if let Some(pipeline) = &args.pipeline {
        query.push_str(&format!("&definitions={}", ado.pipeline_id(ctx, pipeline)?));
    }
    for (key, when) in [("minTime", args.since), ("maxTime", args.until)] {
        if let Some(when) = when {
            query.push_str(&format!("&{key}={}", query_value(&when.utc())));
        }
    }
    if let Some(status) = args.status {
        let status = match status {
            RunStatus::InProgress => "inProgress",
            RunStatus::NotStarted => "notStarted",
            RunStatus::Cancelling => "cancelling",
            RunStatus::Completed => "completed",
            RunStatus::All => "all",
        };
        query.push_str(&format!("&statusFilter={status}"));
    }
    if let Some(result) = args.result {
        let result = match result {
            RunResult::Succeeded => "succeeded",
            RunResult::PartiallySucceeded => "partiallySucceeded",
            RunResult::Failed => "failed",
            RunResult::Canceled => "canceled",
        };
        query.push_str(&format!("&resultFilter={result}"));
    }
    if let Some(who) = &args.requested_by {
        // The builds API takes the person by name, as `az pipelines runs
        // list --requested-for` sends them.
        let who = if who.trim().eq_ignore_ascii_case("@me") {
            ado.me(ctx)?.name
        } else {
            who.trim().to_owned()
        };
        query.push_str(&format!("&requestedFor={}", query_value(&who)));
    }
    if let Some(reason) = args
        .reason
        .and_then(|reason| clap::ValueEnum::to_possible_value(&reason))
    {
        query.push_str(&format!("&reasonFilter={}", reason.get_name()));
    }
    // A bare name is a branch or a tag (images are tagged with the git tag
    // that built them): the branch first, the tag when no branch has runs.
    let refs: Vec<Option<String>> = match args.branch.as_deref().map(str::trim) {
        None => vec![None],
        Some(full) if full.starts_with("refs/") => vec![Some(full.to_owned())],
        Some(name) => vec![Some(full_ref(name)), Some(format!("refs/tags/{name}"))],
    };
    let mut rows: Vec<RunRow> = Vec::new();
    for reference in refs {
        let mut query = query.clone();
        if let Some(reference) = reference {
            query.push_str(&format!("&branchName={}", query_value(&reference)));
        }
        let answer = ado.get(ctx, &ado.code("build/builds", &query))?;
        rows = list(&answer["value"]).iter().map(run_row).collect();
        if !rows.is_empty() {
            break;
        }
    }
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!(
            "[latest {}; --limit N, or narrow with --pipeline --branch --since]",
            args.limit
        ));
    }
    Ok(rows)
}

command! {
    pub RUN_LIST = ["ado", "run", "list"], Read,
    "List pipeline runs (builds), newest first",
    keywords: ["builds", "history", "recent", "latest", "failed", "status", "ci", "tag", "git", "release", "started", "triggered", "queued", "manual", "scheduled", "pr"],
    example: "ado run list --branch refs/tags/v1.4.2 --fields id,pipeline,status,result,finished",
    run: run_list,
}

// ---------- ado run get ----------

#[derive(clap::Args)]
pub struct RunIdArgs {
    /// The run's id: 8812, #8812 or its web URL
    id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunDetail {
    #[serde(flatten)]
    run: RunRow,
    /// The pull request it built, or the one that merged its commit.
    pr: Option<PrRef>,
    /// The work items Azure DevOps associates with the run.
    workitems: Vec<WorkItemRef>,
    /// Tasks running now.
    running: Vec<String>,
    /// What failed: tasks first; jobs or stages only when no task did.
    failed: Vec<FailedStep>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrRef {
    /// What `ado pr get` takes.
    id: i64,
    title: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FailedStep {
    #[serde(rename = "type")]
    kind: Option<String>,
    name: Option<String>,
    /// For `run logs`.
    log_id: Option<i64>,
    errors: Vec<String>,
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

/// The pull request behind a run: the one whose merge commit it built (a CI
/// or tag build of a merged change, or a PR build's own merge), else the
/// number a PR build was triggered for.
fn run_pr(ctx: &Ctx, ado: &Ado, build: &Value) -> Result<Option<PrRef>> {
    let repo = &build["repository"];
    if let (Some(repo_id), Some(commit), Some("TfsGit")) = (
        text(&repo["id"]),
        text(&build["sourceVersion"]),
        repo["type"].as_str(),
    ) {
        let url = ado.code(&format!("git/repositories/{repo_id}/pullrequestquery"), "");
        let body = json!({"queries": [{"type": "lastMergeCommit", "items": [commit]}]});
        let answer = ado.query(ctx, &url, body)?;
        if let Some(pr) = list(&answer["results"])
            .iter()
            .flat_map(|result| list(&result[commit.as_str()]))
            .next()
            && let Some(id) = pr["pullRequestId"].as_i64()
        {
            return Ok(Some(PrRef {
                id,
                title: text(&pr["title"]),
                status: text(&pr["status"]),
            }));
        }
    }
    let triggered = build["triggerInfo"]["pr.number"]
        .as_str()
        .and_then(|number| number.parse().ok())
        .filter(|_| build["reason"].as_str() == Some("pullRequest"));
    Ok(triggered.map(|id| PrRef {
        id,
        title: None,
        status: None,
    }))
}

/// The work items linked to a run, with their titles and states.
fn run_workitems(ctx: &Ctx, ado: &Ado, id: i64) -> Result<Vec<WorkItemRef>> {
    let linked = ado.get(ctx, &ado.code(&format!("build/builds/{id}/workitems"), ""))?;
    let ids: Vec<i64> = list(&linked["value"])
        .iter()
        .filter_map(|item| {
            item["id"]
                .as_str()
                .and_then(|id| id.parse().ok())
                .or_else(|| item["id"].as_i64())
        })
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(workitem::rows(ctx, ado, &ids)?
        .iter()
        .map(WorkItemRef::from)
        .collect())
}

fn run_get(ctx: &Ctx, args: RunIdArgs) -> Result<RunDetail> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.id)?;
    let build = ado.get(ctx, &build_url(&ado, id))?;
    let records = timeline(ctx, &ado, id)?;
    // What produced the run is worth having, not worth failing over.
    let workitems = run_workitems(ctx, &ado, id).unwrap_or_else(|error| {
        ctx.note(format!("[work items not read: {error:#}]"));
        Vec::new()
    });
    let pr = run_pr(ctx, &ado, &build).unwrap_or_else(|error| {
        ctx.note(format!("[pull request not read: {error:#}]"));
        None
    });
    let failed_of = |kind: Option<&str>| -> Vec<FailedStep> {
        records
            .iter()
            .filter(|record| record["result"].as_str() == Some("failed"))
            .filter(|record| kind.is_none_or(|kind| is(record, kind)))
            .map(|record| FailedStep {
                kind: text(&record["type"]),
                name: text(&record["name"]),
                log_id: log_id(record),
                errors: list(&record["issues"])
                    .iter()
                    .filter(|issue| issue["type"].as_str() == Some("error"))
                    .filter_map(|issue| text(&issue["message"]))
                    .collect(),
            })
            .collect()
    };
    let mut failed = failed_of(Some("Task"));
    if failed.is_empty() {
        failed = failed_of(None);
    }
    Ok(RunDetail {
        run: run_row(&build),
        pr,
        workitems,
        running: records
            .iter()
            .filter(|record| is(record, "Task") && record["state"].as_str() == Some("inProgress"))
            .filter_map(|record| text(&record["name"]))
            .collect(),
        failed,
    })
}

command! {
    pub RUN_GET = ["ado", "run", "get"], Read,
    "Show a run: status, commit, timing, what failed, its pull request and work items",
    keywords: ["build", "why", "failed", "broken", "status", "result", "timeline", "errors", "shipped", "deployed", "release"],
    example: "ado run get 1234 --fields status,result,commit,pr,workitems,failed",
    run: run_get,
}

// ---------- ado run logs ----------

#[derive(clap::Args)]
pub struct LogsArgs {
    /// The run's id: 8812, #8812 or its web URL
    id: String,
    /// A job's log, by name
    #[arg(long, conflicts_with = "task")]
    job: Option<String>,
    /// A task's log, by name
    #[arg(long)]
    task: Option<String>,
    /// Lines from the end of each log
    #[arg(long, default_value_t = 200)]
    tail: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunLogs {
    run: i64,
    /// Whose logs these are, in the order `text` holds them.
    logs: Vec<String>,
    text: String,
}

/// The logs to read: the named job or task, else every failed task, else
/// the task running now or the last one that wrote anything.
fn chosen_logs(
    id: i64,
    records: &[Value],
    job: Option<&str>,
    task: Option<&str>,
) -> Result<Vec<(String, i64)>> {
    let named = |record: &Value, name: &str| {
        record["name"]
            .as_str()
            .is_some_and(|held| held.eq_ignore_ascii_case(name.trim()))
    };
    let pick =
        |record: &Value, log: Option<i64>| Some((text(&record["name"]).unwrap_or_default(), log?));
    let picked: Vec<(String, i64)> = match (job, task) {
        (Some(job), _) => records
            .iter()
            .filter(|record| is(record, "Job") && named(record, job))
            .filter_map(|record| {
                // A job whose phase holds the log takes it: in most pipelines
                // that log is the whole job's.
                let phase = records
                    .iter()
                    .find(|parent| is(parent, "Phase") && parent["id"] == record["parentId"]);
                pick(record, log_id(record).or_else(|| phase.and_then(log_id)))
            })
            .collect(),
        (None, Some(task)) => records
            .iter()
            .filter(|record| is(record, "Task") && named(record, task))
            .filter_map(|record| pick(record, log_id(record)))
            .collect(),
        (None, None) => {
            let tasks = || {
                records
                    .iter()
                    .filter(|record| is(record, "Task") && log_id(record).is_some())
            };
            let failed: Vec<(String, i64)> = tasks()
                .filter(|record| record["result"].as_str() == Some("failed"))
                .filter_map(|record| pick(record, log_id(record)))
                .collect();
            if failed.is_empty() {
                tasks()
                    .max_by_key(|record| {
                        (
                            record["state"].as_str() == Some("inProgress"),
                            record["startTime"].as_str().map(str::to_owned),
                        )
                    })
                    .and_then(|record| pick(record, log_id(record)))
                    .into_iter()
                    .collect()
            } else {
                failed
            }
        }
    };
    if picked.is_empty() {
        let (kind, name) = match (job, task) {
            (Some(job), _) => ("Job", Some(job)),
            (None, Some(task)) => ("Task", Some(task)),
            (None, None) => ("Task", None),
        };
        let names: Vec<String> = records
            .iter()
            .filter(|record| is(record, kind))
            .filter_map(|record| text(&record["name"]))
            .take(20)
            .collect();
        let message = match name {
            Some(name) => format!(
                "run {id} has no {} named {name:?} with a log",
                kind.to_lowercase()
            ),
            None => format!("run {id} has written no log yet"),
        };
        return Err(Failure::not_found(message)
            .hint(if names.is_empty() {
                format!("agent-cli ado run get {id} --fields status,result")
            } else {
                format!("its {}s: {}", kind.to_lowercase(), names.join(", "))
            })
            .into());
    }
    Ok(picked)
}

fn run_logs(ctx: &Ctx, args: LogsArgs) -> Result<RunLogs> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.id)?;
    let records = timeline(ctx, &ado, id)?;
    let chosen = chosen_logs(id, &records, args.job.as_deref(), args.task.as_deref())?;
    // The line counts say where each tail starts, so a long log is not read
    // whole to keep its last lines.
    let counts = ado.get(ctx, &ado.code(&format!("build/builds/{}/logs", id), ""))?;
    let mut parts = Vec::new();
    for (name, log) in &chosen {
        let lines = list(&counts["value"])
            .iter()
            .find(|entry| entry["id"].as_i64() == Some(*log))
            .and_then(|entry| entry["lineCount"].as_u64())
            .and_then(|count| usize::try_from(count).ok());
        let start = lines.map_or(0, |count| count.saturating_sub(args.tail));
        let url = ado.code(
            &format!("build/builds/{}/logs/{log}", id),
            &format!("startLine={start}"),
        );
        let answer = ado.get(ctx, &url)?;
        let read: Vec<&str> = list(&answer["value"])
            .iter()
            .filter_map(Value::as_str)
            .collect();
        // Whether startLine counts from 0 or 1 is the service's business;
        // the tail is the tail either way.
        let tail = read[read.len().saturating_sub(args.tail)..].join("\n");
        parts.push(if chosen.len() > 1 {
            format!("--- {name} ---\n{tail}")
        } else {
            tail
        });
    }
    Ok(RunLogs {
        run: id,
        logs: chosen.into_iter().map(|(name, _)| name).collect(),
        text: parts.join("\n"),
    })
}

command! {
    pub RUN_LOGS = ["ado", "run", "logs"], Read,
    "Print the tail of a run's logs: failed tasks by default, or a job or task",
    keywords: ["output", "console", "error", "failed", "build", "tail", "trace"],
    example: "ado run logs 1234 --task 'Run tests' --tail 100",
    run: run_logs,
}

// ---------- ado run create ----------

#[derive(clap::Args)]
pub struct RunCreateArgs {
    /// Pipeline name or id
    #[arg(long)]
    pipeline: String,
    /// The branch to build (default: the pipeline's)
    #[arg(long)]
    branch: Option<String>,
    /// A template parameter as name=value (repeatable)
    #[arg(long)]
    param: Vec<String>,
}

fn run_create(ctx: &Ctx, args: RunCreateArgs) -> Result<RunRow> {
    let ado = Ado::load(ctx)?;
    let mut parameters = Map::new();
    for pair in &args.param {
        let Some((name, value)) = pair.split_once('=') else {
            return Err(Failure::usage(format!("--param takes name=value, not {pair:?}")).into());
        };
        parameters.insert(name.trim().to_owned(), value.into());
    }
    let pipeline = ado.pipeline_id(ctx, &args.pipeline)?;
    let mut body = json!({"templateParameters": parameters});
    if let Some(branch) = &args.branch {
        body["resources"] = json!({"repositories": {"self": {"refName": full_ref(branch)}}});
    }
    let url = ado.code(&format!("pipelines/{pipeline}/runs"), "");
    let run = ado.change(ctx, Effect::Write, Method::Post, &url, body)?;
    // The pipelines endpoint answers in its own shape.
    Ok(RunRow {
        id: run["id"].as_i64().unwrap_or_default(),
        pipeline: text(&run["pipeline"]["name"]),
        pipeline_id: run["pipeline"]["id"].as_i64().or(Some(pipeline)),
        build_number: text(&run["name"]),
        status: text(&run["state"]),
        result: text(&run["result"]),
        branch: run["resources"]["repositories"]["self"]["refName"]
            .as_str()
            .map(short_branch),
        commit: None,
        requested_by: None,
        reason: None,
        queued: stamp(&run["createdDate"]),
        started: None,
        finished: None,
        url: text(&run["_links"]["web"]["href"]),
    })
}

command! {
    pub RUN_CREATE = ["ado", "run", "create"], Write,
    "Start a pipeline run on a branch, with template parameters",
    keywords: ["trigger", "queue", "start", "kick", "off", "build", "deploy", "launch"],
    example: "ado run create --pipeline web-ci --branch 42-fix-login",
    run: run_create,
}

// ---------- ado run wait ----------

fn run_wait(ctx: &Ctx, args: RunIdArgs) -> Result<RunRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.id)?;
    let url = build_url(&ado, id);
    let started = Instant::now();
    loop {
        let response = ado.send(ctx, Method::Get, &url, Body::None, None)?;
        let run = run_row(&response.json()?);
        if run.status.as_deref() == Some("completed") {
            if run.result.as_deref() == Some("succeeded") {
                return Ok(run);
            }
            return Err(Failure::new(
                Exit::Failed,
                format!(
                    "run {id} finished {}",
                    run.result.as_deref().unwrap_or("without a result")
                ),
            )
            .hint(format!(
                "agent-cli ado run get {id} --fields failed, then agent-cli ado run logs {id}"
            ))
            .with_data(run)
            .into());
        }
        let left = ctx.deadline().saturating_duration_since(Instant::now());
        if left <= MARGIN {
            return Err(Failure::timed_out(format!(
                "run {id} is still {} after {}s",
                run.status.as_deref().unwrap_or("going"),
                started.elapsed().as_secs()
            ))
            .hint(format!(
                "run it again to keep waiting (it stays under your shell's 2-minute limit): agent-cli ado run wait {id}"
            ))
            .with_data(run)
            .into());
        }
        // A spent rate-limit budget asks for longer than the usual poll.
        let pause = rate_limit_pause(&response).unwrap_or(POLL);
        std::thread::sleep(pause.min(left - MARGIN));
    }
}

command! {
    pub RUN_WAIT = ["ado", "run", "wait"], Read,
    "Wait for a run to finish: exit 0 if it succeeded, 1 if not, 124 if still going",
    keywords: ["until", "done", "finish", "complete", "block", "poll", "watch", "build"],
    example: "ado run wait 1234 --fields id,status,result",
    timeout: 100,
    run: run_wait,
}

// ---------- ado run cancel / retry ----------

fn run_cancel(ctx: &Ctx, args: RunIdArgs) -> Result<RunRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.id)?;
    let body = json!({"status": "cancelling"});
    let run = ado.change(
        ctx,
        Effect::Destructive,
        Method::Patch,
        &build_url(&ado, id),
        body,
    )?;
    Ok(run_row(&run))
}

command! {
    pub RUN_CANCEL = ["ado", "run", "cancel"], Destructive,
    "Cancel a run that is queued or in progress",
    keywords: ["stop", "abort", "kill", "halt", "build"],
    example: "ado run cancel 1234 --yes",
    run: run_cancel,
}

fn run_retry(ctx: &Ctx, args: RunIdArgs) -> Result<RunRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.id)?;
    let url = ado.code(&format!("build/builds/{}", id), "retry=true");
    let run = ado.change(ctx, Effect::Write, Method::Patch, &url, json!({}))?;
    Ok(run_row(&run))
}

command! {
    pub RUN_RETRY = ["ado", "run", "retry"], Write,
    "Retry the failed jobs of a finished run",
    keywords: ["rerun", "again", "restart", "failed", "build", "flaky"],
    example: "ado run retry 1234",
    run: run_retry,
}

// ---------- approvals ----------

#[derive(clap::Args)]
pub struct ApprovalListArgs {
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApprovalRow {
    /// What approval approve and reject take.
    id: String,
    pipeline: Option<String>,
    run_id: Option<i64>,
    run: Option<String>,
    instructions: Option<String>,
    created: Option<String>,
    approvers: Vec<Approver>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Approver {
    name: Option<String>,
    status: Option<String>,
}

fn approval_list(ctx: &Ctx, args: ApprovalListArgs) -> Result<Vec<ApprovalRow>> {
    let ado = Ado::load(ctx)?;
    let url = ado.api(
        Some(&ado.code_project),
        "pipelines/approvals",
        &format!("state=pending&$expand=steps&top={}", args.limit + 1),
        PREVIEW_API,
    );
    let answer = ado.get(ctx, &url)?;
    let mut rows: Vec<ApprovalRow> = list(&answer["value"])
        .iter()
        .filter_map(|approval| {
            let owner = &approval["pipeline"]["owner"];
            Some(ApprovalRow {
                id: text(&approval["id"])?,
                pipeline: text(&approval["pipeline"]["name"]),
                run_id: owner["id"].as_i64(),
                run: text(&owner["name"]),
                instructions: text(&approval["instructions"]),
                created: stamp(&approval["createdOn"]),
                approvers: list(&approval["steps"])
                    .iter()
                    .map(|step| Approver {
                        name: text(&step["assignedApprover"]["displayName"]),
                        status: text(&step["status"]),
                    })
                    .collect(),
            })
        })
        .collect();
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!("[first {}; --limit N for more]", args.limit));
    }
    Ok(rows)
}

command! {
    pub APPROVAL_LIST = ["ado", "approval", "list"], Read,
    "List pending pipeline approvals (deployment gates)",
    keywords: ["pending", "waiting", "deploy", "release", "checks", "stage", "blocked"],
    example: "ado approval list --fields id,pipeline,run,approvers",
    run: approval_list,
}

#[derive(clap::Args)]
pub struct AnswerArgs {
    /// The approval's id, from approval list
    id: String,
    /// Why, for the record
    #[arg(long)]
    comment: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Answered {
    id: String,
    /// approved or rejected, as Azure DevOps recorded it.
    status: Option<String>,
}

fn answer(ctx: &Ctx, args: AnswerArgs, status: &str) -> Result<Answered> {
    let ado = Ado::load(ctx)?;
    let url = ado.api(
        Some(&ado.code_project),
        "pipelines/approvals",
        "",
        PREVIEW_API,
    );
    let body = json!([{
        "approvalId": args.id,
        "status": status,
        "comment": args.comment.unwrap_or_default(),
    }]);
    let answered = ado.change(ctx, Effect::Destructive, Method::Patch, &url, body)?;
    Ok(Answered {
        id: args.id,
        status: text(&answered["value"][0]["status"]),
    })
}

fn approval_approve(ctx: &Ctx, args: AnswerArgs) -> Result<Answered> {
    answer(ctx, args, "approved")
}

fn approval_reject(ctx: &Ctx, args: AnswerArgs) -> Result<Answered> {
    answer(ctx, args, "rejected")
}

command! {
    pub APPROVAL_APPROVE = ["ado", "approval", "approve"], Destructive,
    "Approve a pending pipeline approval, letting the stage (a deploy) run",
    keywords: ["gate", "deploy", "release", "allow", "production", "sign", "off"],
    example: "ado approval approve 1f2e3d4c-0000-4000-8000-000000000001 --comment 'Checked staging' --yes",
    run: approval_approve,
}

command! {
    pub APPROVAL_REJECT = ["ado", "approval", "reject"], Destructive,
    "Reject a pending pipeline approval, stopping the stage",
    keywords: ["gate", "deploy", "release", "deny", "block", "production"],
    example: "ado approval reject 1f2e3d4c-0000-4000-8000-000000000001 --comment 'Not today' --yes",
    run: approval_reject,
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testkit::{ado, dry_run, urls};

    const CODE: &str = "https://dev.azure.com/contoso/Fabrikam/_apis";

    fn build(id: i64, status: &str, result: Option<&str>) -> Value {
        json!({
            "id": id,
            "buildNumber": "20260929.3",
            "status": status,
            "result": result,
            "definition": {"id": 12, "name": "web-ci"},
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "abc123",
            "requestedFor": {"displayName": "Jane Doe"},
            "reason": "individualCI",
            "queueTime": "2026-09-29T10:00:00Z",
            "startTime": "2026-09-29T10:00:05Z",
            "_links": {"web": {"href": format!("https://dev.azure.com/contoso/Fabrikam/_build/results?buildId={id}")}}
        })
    }

    fn page(items: Vec<Value>) -> Answer {
        Answer::json(&json!({"count": items.len(), "value": items}))
    }

    fn record(
        id: &str,
        kind: &str,
        name: &str,
        parent: Option<&str>,
        result: Option<&str>,
        log: i64,
    ) -> Value {
        json!({"id": id, "parentId": parent, "type": kind, "name": name, "state": "completed",
            "result": result, "log": if log > 0 { json!({"id": log}) } else { Value::Null },
            "startTime": format!("2026-09-29T10:00:{log:02}Z"), "issues": []})
    }

    fn timeline() -> Answer {
        let mut failed = record("t2", "Task", "Run tests", Some("j1"), Some("failed"), 5);
        failed["issues"] = json!([
            {"type": "error", "message": "3 tests failed"},
            {"type": "warning", "message": "slow"}
        ]);
        Answer::json(&json!({"records": [
            record("s1", "Stage", "Build", None, Some("failed"), 0),
            record("p1", "Phase", "Job", Some("s1"), Some("failed"), 2),
            record("j1", "Job", "Linux", Some("p1"), Some("failed"), 0),
            record("t1", "Task", "Restore", Some("j1"), Some("succeeded"), 4),
            failed,
        ]}))
    }

    #[test]
    fn pipeline_list_filters_by_repo_and_shows_the_last_run() {
        let (outcome, transport) = ado(
            &[
                "ado", "pipeline", "list", "web", "--repo", "web", "--limit", "1",
            ],
            vec![
                page(vec![
                    json!({"id": "r-1", "name": "web", "project": {"id": "p-1"}}),
                ]),
                page(vec![
                    json!({"id": 12, "name": "web-ci", "path": "\\", "queueStatus": "enabled",
                        "latestBuild": {"id": 991, "status": "completed", "result": "failed",
                            "sourceBranch": "refs/heads/main", "finishTime": "2026-09-29T10:09:00Z"},
                        "_links": {"web": {"href": "https://dev.azure.com/contoso/Fabrikam/_build/definition?definitionId=12"}}}),
                    json!({"id": 13, "name": "web-nightly"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0]["last_run"],
            json!({"id": 991, "status": "completed", "result": "failed", "branch": "main",
                "finished": "2026-09-29T10:09:00Z"})
        );
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert!(
            outcome.stderr.starts_with("[first 1;"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{CODE}/build/definitions?includeLatestBuilds=true&queryOrder=definitionNameAscending&$top=2&name=%2Aweb%2A&repositoryId=r-1&repositoryType=TfsGit&api-version=7.1"
            )
        );
    }

    #[test]
    fn run_list_filters_by_who_queued_it_and_why() {
        let me = Answer::json(&json!({"authenticatedUser": {"id": "u-1",
            "providerDisplayName": "Jane Doe", "properties": {"Account": {"$value": "jane@contoso.com"}}}}));
        let (outcome, transport) = ado(
            &[
                "ado",
                "run",
                "list",
                "--requested-by",
                "@me",
                "--reason",
                "manual",
                "--since",
                "2026-09-29",
                "--fields",
                "id,requested_by,reason",
            ],
            vec![me, page(vec![build(991, "completed", Some("succeeded"))])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": 991, "requested_by": "Jane Doe", "reason": "individualCI"}])
        );
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{CODE}/build/builds?queryOrder=queueTimeDescending&$top=51&minTime=2026-09-29T00%3A00%3A00Z&requestedFor=Jane+Doe&reasonFilter=manual&api-version=7.1"
            )
        );

        let (outcome, transport) = ado(
            &[
                "ado",
                "run",
                "list",
                "--requested-by",
                "sam@contoso.com",
                "--reason",
                "individualci",
            ],
            vec![page(vec![])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            urls(&transport)[0]
                .contains("&requestedFor=sam%40contoso.com&reasonFilter=individualCI&"),
            "{:?}",
            urls(&transport)
        );
        let (outcome, _) = ado(&["ado", "run", "list", "--reason", "push"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn run_list_resolves_the_pipeline_by_name_and_filters_server_side() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "run",
                "list",
                "--pipeline",
                "WEB-CI",
                "--branch",
                "main",
                "--status",
                "completed",
                "--result",
                "partiallySucceeded",
                "--limit",
                "1",
            ],
            vec![
                page(vec![
                    json!({"id": 12, "name": "web-ci", "path": "\\"}),
                    json!({"id": 14, "name": "web-ci-2"}),
                ]),
                page(vec![
                    build(991, "completed", Some("partiallySucceeded")),
                    build(990, "completed", Some("partiallySucceeded")),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["id"], 991);
        assert_eq!(rows[0]["pipeline"], "web-ci");
        assert_eq!(rows[0]["branch"], "main");
        assert_eq!(rows[0]["requested_by"], "Jane Doe");
        assert!(
            outcome.stderr.starts_with("[latest 1;"),
            "{}",
            outcome.stderr
        );
        let sent = urls(&transport);
        assert_eq!(
            sent[0],
            format!("{CODE}/build/definitions?name=WEB-CI&api-version=7.1")
        );
        assert_eq!(
            sent[1],
            format!(
                "{CODE}/build/builds?queryOrder=queueTimeDescending&$top=2&definitions=12&statusFilter=completed&resultFilter=partiallySucceeded&branchName=refs%2Fheads%2Fmain&api-version=7.1"
            )
        );

        let (outcome, _) = ado(
            &["ado", "run", "list", "--pipeline", "nope"],
            vec![page(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli ado pipeline list nope"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_bare_name_is_a_branch_then_a_tag_and_a_window_is_minimum_and_maximum_time() {
        let mut tagged = build(8812, "completed", Some("succeeded"));
        tagged["sourceBranch"] = json!("refs/tags/v1.4.2");
        let (outcome, transport) = ado(
            &[
                "ado",
                "run",
                "list",
                "--branch",
                "v1.4.2",
                "--since",
                "2026-09-28",
                "--until",
                "2026-09-29T12:00:00+02:00",
            ],
            vec![page(vec![]), page(vec![tagged.clone()])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[0]["branch"],
            "refs/tags/v1.4.2",
            "a tag keeps its refs/tags/, so it pastes back into --branch"
        );
        let window = "minTime=2026-09-28T00%3A00%3A00Z&maxTime=2026-09-29T10%3A00%3A00Z";
        assert_eq!(
            urls(&transport),
            [
                format!(
                    "{CODE}/build/builds?queryOrder=queueTimeDescending&$top=51&{window}&branchName=refs%2Fheads%2Fv1.4.2&api-version=7.1"
                ),
                format!(
                    "{CODE}/build/builds?queryOrder=queueTimeDescending&$top=51&{window}&branchName=refs%2Ftags%2Fv1.4.2&api-version=7.1"
                ),
            ]
        );

        let (outcome, transport) = ado(
            &["ado", "run", "list", "--branch", "refs/tags/v1.4.2"],
            vec![page(vec![tagged])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(transport.sent().len(), 1, "a full ref is asked for once");
    }

    #[test]
    fn run_get_says_which_pull_request_and_work_items_produced_it() {
        let mut built = build(8812, "completed", Some("succeeded"));
        built["sourceBranch"] = json!("refs/tags/v1.4.2");
        built["sourceVersion"] = json!("4be1c0d");
        built["repository"] = json!({"id": "r-1", "type": "TfsGit", "name": "api"});
        let item = |id: i64, title: &str| {
            json!({"id": id, "rev": 3, "fields": {"System.WorkItemType": "User Story",
                "System.Title": title, "System.State": "Resolved"}})
        };
        let (outcome, transport) = ado(
            &[
                "ado",
                "run",
                "get",
                "https://dev.azure.com/contoso/Fabrikam/_build/results?buildId=8812&view=results",
            ],
            vec![
                Answer::json(&built),
                Answer::json(&json!({"records": []})),
                page(vec![
                    json!({"id": "1207", "url": "x"}),
                    json!({"id": "1210", "url": "x"}),
                ]),
                page(vec![
                    item(1210, "Log the retry count"),
                    item(1207, "Throttle handling"),
                ]),
                Answer::json(&json!({"results": [{"4be1c0d": [
                    {"pullRequestId": 431, "title": "Retry on 429", "status": "completed"}]}]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], 8812);
        assert_eq!(got["commit"], "4be1c0d");
        assert_eq!(
            got["pr"],
            json!({"id": 431, "title": "Retry on 429", "status": "completed"})
        );
        assert_eq!(
            got["workitems"],
            json!([
                {"id": 1207, "type": "User Story", "title": "Throttle handling", "state": "Resolved"},
                {"id": 1210, "type": "User Story", "title": "Log the retry count", "state": "Resolved"}
            ])
        );
        let sent = transport.sent();
        assert_eq!(
            sent[2].url,
            format!("{CODE}/build/builds/8812/workitems?api-version=7.1")
        );
        assert_eq!(
            sent[4].url,
            format!("{CODE}/git/repositories/r-1/pullrequestquery?api-version=7.1")
        );
        assert!(sent[4].method.is_read(), "a query that only reads");
        assert_eq!(
            sent[4].body.as_ref().unwrap(),
            &json!({"queries": [{"type": "lastMergeCommit", "items": ["4be1c0d"]}]})
        );

        let mut triggered = build(8813, "completed", Some("failed"));
        triggered["reason"] = json!("pullRequest");
        triggered["triggerInfo"] = json!({"pr.number": "432"});
        let (outcome, _) = ado(
            &["ado", "run", "get", "#8813"],
            vec![
                Answer::json(&triggered),
                Answer::json(&json!({"records": []})),
                Answer::status(500, r#"{"message":"work items unavailable"}"#),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["pr"], json!({"id": 432}));
        assert!(
            outcome.stderr.contains("[work items not read:"),
            "{}",
            outcome.stderr
        );

        let (outcome, transport) = ado(
            &[
                "ado",
                "run",
                "get",
                "https://dev.azure.com/fabrikam/X/_build/results?buildId=1",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn run_get_names_the_failed_tasks_and_their_errors() {
        let (outcome, transport) = ado(
            &["ado", "run", "get", "991"],
            vec![
                Answer::json(&build(991, "completed", Some("failed"))),
                timeline(),
                page(vec![]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["result"], "failed");
        assert_eq!(got["commit"], "abc123");
        assert_eq!(
            got["failed"],
            json!([{"type": "Task", "name": "Run tests", "log_id": 5, "errors": ["3 tests failed"]}])
        );
        assert_eq!(
            urls(&transport)[1],
            format!("{CODE}/build/builds/991/timeline?api-version=7.1")
        );
    }

    #[test]
    fn run_get_of_a_queued_run_has_no_timeline_but_a_broken_one_is_an_error() {
        let (outcome, _) = ado(
            &["ado", "run", "get", "992"],
            vec![
                Answer::json(&build(992, "notStarted", None)),
                Answer::ok(""),
                page(vec![]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["status"], "notStarted");

        let (outcome, _) = ado(
            &["ado", "run", "get", "993"],
            vec![
                Answer::json(&build(993, "completed", Some("failed"))),
                Answer::status(500, r#"{"message":"timeline unavailable"}"#),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("timeline unavailable"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn logs_tail_the_failed_tasks_from_the_line_count() {
        let lines: Vec<String> = (1..=3).map(|n| format!("line {n}")).collect();
        let (outcome, transport) = ado(
            &["ado", "run", "logs", "991", "--tail", "2"],
            vec![
                timeline(),
                page(vec![
                    json!({"id": 5, "lineCount": 900}),
                    json!({"id": 4, "lineCount": 10}),
                ]),
                page(lines.iter().map(|line| json!(line)).collect()),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"run": 991, "logs": ["Run tests"], "text": "line 2\nline 3"})
        );
        assert_eq!(
            urls(&transport)[2],
            format!("{CODE}/build/builds/991/logs/5?startLine=898&api-version=7.1")
        );
    }

    #[test]
    fn a_named_job_takes_its_phases_log_and_an_unknown_one_lists_the_jobs() {
        let (outcome, transport) = ado(
            &["ado", "run", "logs", "991", "--job", "linux"],
            vec![timeline(), page(vec![]), page(vec![json!("job output")])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["text"], "job output");
        assert_eq!(
            urls(&transport)[2],
            format!("{CODE}/build/builds/991/logs/2?startLine=0&api-version=7.1")
        );

        let (outcome, _) = ado(
            &["ado", "run", "logs", "991", "--task", "Deploy"],
            vec![timeline()],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: its tasks: Restore, Run tests"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = ado(&["ado", "run", "logs", "992"], vec![Answer::ok("")]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("has written no log yet"),
            "{}",
            outcome.stderr
        );
        let (outcome, _) = ado(
            &["ado", "run", "logs", "1", "--job", "a", "--task", "b"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn with_nothing_failed_the_logs_are_the_last_tasks() {
        let answer = json!({"records": [
            record("t1", "Task", "Restore", None, Some("succeeded"), 4),
            record("t2", "Task", "Build", None, Some("succeeded"), 6),
        ]});
        let (outcome, _) = ado(
            &["ado", "run", "logs", "991"],
            vec![
                Answer::json(&answer),
                page(vec![]),
                page(vec![json!("built")]),
            ],
        );
        assert_eq!(outcome.json()["logs"], json!(["Build"]));
    }

    #[test]
    fn create_resolves_the_pipeline_then_plans_the_run_with_its_parameters() {
        let plans = dry_run(
            &[
                "ado",
                "run",
                "create",
                "--pipeline",
                "web-ci",
                "--branch",
                "42-fix",
                "--param",
                "env=staging",
                "--param",
                "debug=",
            ],
            vec![page(vec![json!({"id": 12, "name": "web-ci"})])],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/pipelines/12/runs?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"templateParameters": {"env": "staging", "debug": ""},
                "resources": {"repositories": {"self": {"refName": "refs/heads/42-fix"}}}})
        );

        let (outcome, _) = ado(
            &["ado", "run", "create", "--pipeline", "12"],
            vec![Answer::json(
                &json!({"id": 995, "name": "20260929.4", "state": "inProgress",
                "pipeline": {"id": 12, "name": "web-ci"}, "createdDate": "2026-09-29T11:00:00Z",
                "resources": {"repositories": {"self": {"refName": "refs/heads/main"}}}}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": 995, "pipeline": "web-ci", "pipeline_id": 12, "build_number": "20260929.4",
                "status": "inProgress", "branch": "main", "queued": "2026-09-29T11:00:00Z"})
        );
        let (outcome, _) = ado(
            &[
                "ado",
                "run",
                "create",
                "--pipeline",
                "12",
                "--param",
                "oops",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn wait_exits_0_when_it_succeeded_and_1_with_the_state_when_it_did_not() {
        let (outcome, _) = ado(
            &["ado", "run", "wait", "991"],
            vec![Answer::json(&build(991, "completed", Some("succeeded")))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["result"], "succeeded");

        let (outcome, _) = ado(
            &["ado", "run", "wait", "991"],
            vec![Answer::json(&build(991, "completed", Some("failed")))],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(outcome.json()["result"], "failed", "the run it waited on");
        assert!(
            outcome
                .stderr
                .starts_with("error: run 991 finished failed\n"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli ado run get 991 --fields failed"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn wait_at_the_deadline_is_124_with_the_current_state() {
        let started = Instant::now();
        let (outcome, transport) = ado(
            &["ado", "run", "wait", "991", "--timeout", "3"],
            vec![
                Answer::json(&build(991, "inProgress", None)),
                Answer::json(&build(991, "inProgress", None)),
            ],
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert_eq!(
            transport.sent().len(),
            2,
            "polled, slept to the margin, polled once more"
        );
        assert!(
            outcome.stderr.contains("run 991 is still inProgress after"),
            "{}",
            outcome.stderr
        );
        assert_eq!(outcome.json()["status"], "inProgress", "its current state");
        assert!(
            outcome
                .stderr
                .contains("to keep waiting (it stays under your shell's 2-minute limit): agent-cli ado run wait 991\n"),
            "{}",
            outcome.stderr
        );
    }

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

    #[test]
    fn approvals_list_the_pending_gates_and_answering_one_is_destructive() {
        let (outcome, transport) = ado(
            &["ado", "approval", "list"],
            vec![page(vec![
                json!({"id": "a-1", "status": "pending", "instructions": "Check staging",
                "createdOn": "2026-09-29T10:10:00Z",
                "pipeline": {"id": 12, "name": "web-deploy", "owner": {"id": 991, "name": "20260929.3"}},
                "steps": [{"assignedApprover": {"displayName": "Release Managers"}, "status": "pending"}]}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "a-1", "pipeline": "web-deploy", "run_id": 991, "run": "20260929.3",
                "instructions": "Check staging", "created": "2026-09-29T10:10:00Z",
                "approvers": [{"name": "Release Managers", "status": "pending"}]}])
        );
        assert_eq!(
            urls(&transport),
            [format!(
                "{CODE}/pipelines/approvals?state=pending&$expand=steps&top=51&api-version=7.1-preview.1"
            )]
        );

        let plans = dry_run(
            &["ado", "approval", "approve", "a-1", "--comment", "Checked"],
            vec![],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/pipelines/approvals?api-version=7.1-preview.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!([{"approvalId": "a-1", "status": "approved", "comment": "Checked"}])
        );

        let (outcome, _) = ado(
            &["ado", "approval", "reject", "a-1", "--yes"],
            vec![page(vec![json!({"id": "a-1", "status": "rejected"})])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!({"id": "a-1", "status": "rejected"}));
        let (outcome, _) = ado(&["ado", "approval", "approve", "a-1"], vec![]);
        assert_eq!(outcome.code, 2, "a gate needs --yes: {outcome:?}");
    }
}
