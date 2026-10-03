//! Review threads on a pull request. `thread list` and `pr get` both print
//! them with the same `id` (`PR/THREAD`) and `at`, so either listing hands
//! the thread verbs and `file get` what they take.

pub(crate) mod comment;
pub(crate) mod list;
pub(crate) mod update;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use serde_json::Value;

use crate::client::{Ado, list, text};
use crate::ids::{file_id, thread_id};
use crate::pr::{fetch_pr, latest_iteration, pr_home};

/// A pull request's threads, placed on its latest iteration: a comment's
/// lines are where they are at the head commit, however many pushes came
/// after it.
pub(crate) struct Threads {
    pub(crate) all: Vec<Value>,
    repo: String,
    /// What right-side (new) lines are at: the head commit.
    head: Option<String>,
    /// What left-side (deleted) lines are at: the merge base.
    base: Option<String>,
}

/// Where a thread's comment is.
pub(crate) struct Place {
    /// Without a leading `/`.
    pub(crate) file: String,
    pub(crate) line: Option<usize>,
    pub(crate) end: Option<usize>,
    pub(crate) commit: Option<String>,
    /// The `file get` id of those lines at that commit.
    pub(crate) at: Option<String>,
}

pub(crate) fn fetch_threads(ctx: &Ctx, ado: &Ado, pr: &Value, id: i64) -> Result<Threads> {
    let (repo_id, _) = pr_home(pr)?;
    let latest = latest_iteration(ctx, ado, &repo_id, id)?;
    let query = latest["id"]
        .as_i64()
        .map(|iteration| format!("$iteration={iteration}"))
        .unwrap_or_default();
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullRequests/{id}/threads"),
        &query,
    );
    let answer = ado.get(ctx, &url)?;
    Ok(Threads {
        all: list(&answer["value"]).to_vec(),
        repo: text(&pr["repository"]["name"]).unwrap_or(repo_id),
        head: text(&latest["sourceRefCommit"]["commitId"]),
        base: text(&latest["commonRefCommit"]["commitId"]),
    })
}

impl Threads {
    /// Where `thread` is, or `None` for one on the whole pull request.
    pub(crate) fn place(&self, thread: &Value) -> Option<Place> {
        let context = &thread["threadContext"];
        let file = text(&context["filePath"])?
            .trim_start_matches('/')
            .to_owned();
        let (side, commit) = if context["rightFileStart"].is_object() {
            ("right", &self.head)
        } else {
            ("left", &self.base)
        };
        let line = |at: &str| {
            context[format!("{side}File{at}")]["line"]
                .as_u64()
                .and_then(|line| usize::try_from(line).ok())
                .filter(|line| *line > 0)
        };
        let (line, end) = (line("Start"), line("End"));
        let lines = line.map(|first| (first, end.unwrap_or(first).max(first)));
        Some(Place {
            at: commit
                .as_deref()
                .map(|commit| file_id(None, &self.repo, Some(commit), &file, lines)),
            commit: commit.clone(),
            end: lines.map(|(_, end)| end),
            line,
            file,
        })
    }
}

/// Discussion, not what Azure DevOps wrote about itself (a vote, a push).
pub(crate) fn is_discussion(thread: &Value) -> bool {
    !thread["isDeleted"].as_bool().unwrap_or_default()
        && thread["comments"][0]["commentType"].as_str() != Some("system")
}

/// The thread an id names (`436/7`, its URL, or a number with `--pr`), and
/// its URL path under the code project.
pub(crate) fn locate(
    ctx: &Ctx,
    ado: &Ado,
    raw: &str,
    pr: Option<&str>,
) -> Result<(String, String)> {
    let (pr, thread) = thread_id(ado, raw, pr)?;
    let (repo_id, _) = pr_home(&fetch_pr(ctx, ado, pr)?)?;
    Ok((
        format!("{pr}/{thread}"),
        format!("git/repositories/{repo_id}/pullRequests/{pr}/threads/{thread}"),
    ))
}

/// Thread `id` (`PR/THREAD`) is not there. Azure DevOps says so with a 200
/// and `null` to a PATCH, and a 500 to a reply.
pub(crate) fn no_thread(id: &str) -> anyhow::Error {
    let pr = id.split('/').next().unwrap_or(id);
    Failure::not_found(format!("there is no thread {id}"))
        .hint(format!("agent-cli ado thread list {pr}"))
        .into()
}

/// A thread's status, as Azure DevOps writes it.
#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum Status {
    Active,
    Fixed,
    #[value(name = "wontFix")]
    WontFix,
    Closed,
    #[value(name = "byDesign")]
    ByDesign,
    Pending,
}

/// The name clap reads and Azure DevOps writes.
pub(crate) fn wire(value: impl clap::ValueEnum) -> String {
    value
        .to_possible_value()
        .map(|value| value.get_name().to_owned())
        .unwrap_or_default()
}
