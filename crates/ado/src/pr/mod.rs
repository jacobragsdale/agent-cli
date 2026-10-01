//! Pull requests. Every pull request the filters match is listed:
//! ticket-tui showed only repositories with a clone on the machine.

pub(crate) mod abandon;
pub(crate) mod comment;
pub(crate) mod complete;
pub(crate) mod create;
pub(crate) mod get;
pub(crate) mod link;
pub(crate) mod list;
pub(crate) mod update;
pub(crate) mod vote;

use agent_cli_core::{Ctx, Failure};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Ado, list, short_branch, stamp, text};
use crate::work_items::add_artifact_link;

// ---------- pull request rows ----------

/// One pull request as a list shows it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PrRow {
    id: i64,
    repo: Option<String>,
    title: Option<String>,
    author: Option<String>,
    /// active, completed or abandoned.
    status: Option<String>,
    is_draft: bool,
    source: Option<String>,
    target: Option<String>,
    /// succeeded, conflicts, queued …
    merge_status: Option<String>,
    /// Who set auto-complete, when it is on.
    auto_complete: Option<String>,
    created: Option<String>,
    reviewers: Vec<Reviewer>,
    url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Reviewer {
    name: String,
    /// approved, suggestions, none, waiting or rejected.
    vote: &'static str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    required: bool,
}

/// A vote on the API's own scale, in words.
fn vote_word(vote: i64) -> &'static str {
    match vote {
        10 => "approved",
        5 => "suggestions",
        -5 => "waiting",
        -10 => "rejected",
        _ => "none",
    }
}

fn pr_row(ado: &Ado, pr: &Value) -> PrRow {
    let id = pr["pullRequestId"].as_i64().unwrap_or_default();
    let repo = text(&pr["repository"]["name"]);
    let project =
        text(&pr["repository"]["project"]["name"]).unwrap_or_else(|| ado.code_project.clone());
    PrRow {
        id,
        url: repo
            .as_deref()
            .map(|repo| ado.pull_request_url(&project, repo, id)),
        repo,
        title: text(&pr["title"]),
        author: text(&pr["createdBy"]["displayName"]),
        status: text(&pr["status"]),
        is_draft: pr["isDraft"].as_bool().unwrap_or_default(),
        source: pr["sourceRefName"].as_str().map(short_branch),
        target: pr["targetRefName"].as_str().map(short_branch),
        merge_status: text(&pr["mergeStatus"]),
        auto_complete: text(&pr["autoCompleteSetBy"]["displayName"]),
        created: stamp(&pr["creationDate"]),
        reviewers: list(&pr["reviewers"])
            .iter()
            .filter_map(|reviewer| {
                Some(Reviewer {
                    name: text(&reviewer["displayName"])?,
                    vote: vote_word(reviewer["vote"].as_i64().unwrap_or_default()),
                    required: reviewer["isRequired"].as_bool().unwrap_or_default(),
                })
            })
            .collect(),
    }
}

/// The pull request as stored, found by id alone: the project-wide endpoint
/// needs no repository.
pub(crate) fn fetch_pr(ctx: &Ctx, ado: &Ado, id: i64) -> Result<Value> {
    ado.get(ctx, &ado.code(&format!("git/pullrequests/{id}"), ""))
}

/// The repository id and project id a pull request lives under.
pub(crate) fn pr_home(pr: &Value) -> Result<(String, String)> {
    let repo = text(&pr["repository"]["id"])
        .context("the pull request came back without its repository")?;
    let project = text(&pr["repository"]["project"]["id"])
        .context("the pull request came back without its project")?;
    Ok((repo, project))
}

/// A pull request's latest iteration (its latest push): `sourceRefCommit`
/// is its head and `commonRefCommit` its merge base with the target. `null`
/// for one with none.
pub(crate) fn latest_iteration(ctx: &Ctx, ado: &Ado, repo_id: &str, id: i64) -> Result<Value> {
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullRequests/{id}/iterations"),
        "",
    );
    let answer = ado.get(ctx, &url)?;
    Ok(list(&answer["value"])
        .iter()
        .max_by_key(|iteration| iteration["id"].as_i64())
        .cloned()
        .unwrap_or_default())
}

/// The work items a pull request says it closes.
fn pr_work_items(ctx: &Ctx, ado: &Ado, repo_id: &str, id: i64) -> Result<Vec<i64>> {
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullRequests/{id}/workitems"),
        "",
    );
    let answer = ado.get(ctx, &url)?;
    Ok(list(&answer["value"])
        .iter()
        .filter_map(|entry| {
            entry["id"]
                .as_str()
                .and_then(|id| id.parse().ok())
                .or_else(|| entry["id"].as_i64())
        })
        .collect())
}

/// Links work item `work_item` to pull request `id`: the link lives on the
/// work item, as an artifact link naming the pull request. Returns whether
/// it wrote one (false: it was already there).
fn link_pr(
    ctx: &Ctx,
    ado: &Ado,
    project_id: &str,
    repo_id: &str,
    id: i64,
    work_item: i64,
) -> Result<bool> {
    let url = format!("vstfs:///Git/PullRequestId/{project_id}%2F{repo_id}%2F{id}");
    add_artifact_link(ctx, ado, work_item, &url, "Pull Request")
}

/// The pull request as it is now, refusing one that is no longer active.
fn active_pr(ctx: &Ctx, ado: &Ado, id: i64) -> Result<(Value, String)> {
    let pr = fetch_pr(ctx, ado, id)?;
    let status = pr["status"].as_str().unwrap_or_default();
    if status != "active" {
        return Err(
            Failure::conflict(format!("pull request {id} is already {status}"))
                .hint(format!("agent-cli ado pr get {id} --fields status,url"))
                .into(),
        );
    }
    let (repo_id, _) = pr_home(&pr)?;
    Ok((pr, repo_id))
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{ado_piped, dry_run, page, pr, repos};

    #[test]
    fn a_description_or_comment_comes_piped_with_a_dash_or_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let why = dir.path().join("why.md");
        std::fs::write(&why, "Fixes **login**\r\n\r\n- on Safari\r\n").unwrap();
        let why = why.to_str().unwrap();
        let plans = dry_run(
            &[
                "ado",
                "pr",
                "create",
                "--repo",
                "web",
                "--source",
                "42-fix-login",
                "--title",
                "Fix login",
                "--description-file",
                why,
            ],
            vec![repos(), page(vec![])],
        );
        assert_eq!(
            plans[0]["body"]["description"],
            "Fixes **login**\n\n- on Safari"
        );

        let (outcome, _) = ado_piped(
            "Now also **Firefox**\n",
            &[
                "ado",
                "pr",
                "update",
                "17",
                "--description",
                "-",
                "--dry-run",
            ],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["would"][0]["body"],
            json!({"description": "Now also **Firefox**"})
        );

        let plans = dry_run(
            &["ado", "pr", "comment", "17", "--text-file", why],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(
            plans[0]["body"]["comments"][0]["content"], "Fixes **login**\n\n- on Safari",
            "a file is Markdown, not a code block"
        );
        let (outcome, _) = ado_piped(
            "ok 1\n",
            &["ado", "pr", "comment", "17", "-", "--dry-run"],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(
            outcome.json()["would"][0]["body"]["comments"][0]["content"],
            "```\nok 1\n```"
        );
    }
}
