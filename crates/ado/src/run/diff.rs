//! `ado run diff`: what is in this deploy. Two runs of one pipeline, each
//! as `run get` describes it, and on the newer side the commits, pull
//! requests and work items Azure DevOps counts between them (`build/changes`
//! and `build/workitems` from one build to the other).

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, DiffArgs, Failure, Rules, Side, command, diff};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::{Ado, Kind, list, text};
use crate::work_items::{self, WorkItemRef};

use super::{build_url, run_row};

#[derive(clap::Args)]
pub struct RunDiffArgs {
    /// One run: 8790, #8790 or its web URL
    left: String,
    /// The other run of the same pipeline
    right: String,
    #[command(flatten)]
    diff: DiffArgs,
}

/// Two runs are no two environments; each has its own times and links.
const RULES: Rules = Rules {
    noise: &["id", "build_number", "queued", "started", "finished", "url"],
    infer: false,
    ..Rules::PLAIN
};

/// The changes between two builds, as the service pages them (checked on
/// Azure DevOps Services).
const BETWEEN_API: &str = "7.1-preview.2";
const MOST: usize = 200;

fn run_diff(ctx: &Ctx, args: RunDiffArgs) -> Result<Diff> {
    let ado = Ado::load(ctx)?;
    let (left, right) = (
        ado.id(Kind::Run, &args.left)?,
        ado.id(Kind::Run, &args.right)?,
    );
    let read = |id| ado.get(ctx, &build_url(&ado, id));
    let (left_build, right_build) = (read(left)?, read(right)?);
    let pipeline = |build: &Value| build["definition"]["id"].as_i64();
    if pipeline(&left_build) != pipeline(&right_build) {
        return Err(Failure::usage(format!(
            "run {left} is of {} and run {right} of {}: what lies between runs is counted within one pipeline",
            text(&left_build["definition"]["name"]).unwrap_or_default(),
            text(&right_build["definition"]["name"]).unwrap_or_default()
        ))
        .into());
    }
    let (from, to) = (left.min(right), left.max(right));
    let between = format!("fromBuildId={from}&toBuildId={to}&$top={MOST}");
    // From a build to itself the service answers that build's own changes.
    let read_between = |path: &str| -> Result<Value> {
        if from == to {
            return Ok(Value::Null);
        }
        ado.get(
            ctx,
            &ado.api(Some(&ado.code_project), path, &between, BETWEEN_API),
        )
    };
    let (changes, linked) = (
        read_between("build/changes")?,
        read_between("build/workitems")?,
    );
    let mut commits = BTreeMap::new();
    let mut prs = BTreeMap::new();
    for change in list(&changes["value"]) {
        let (Some(sha), Some(message)) = (text(&change["id"]), text(&change["message"])) else {
            continue;
        };
        let first = message.lines().next().unwrap_or_default().to_owned();
        if let Some((number, title)) = merged(&first) {
            prs.insert(number.to_owned(), json!(title));
        }
        let author = text(&change["author"]["displayName"]).unwrap_or_default();
        commits.insert(sha, json!(format!("{first} ({author})")));
    }
    if list(&changes["value"]).len() >= MOST {
        ctx.note(format!("[the first {MOST} commits between the runs]"));
    }
    let ids: Vec<i64> = list(&linked["value"])
        .iter()
        .filter_map(|item| {
            text(&item["id"])
                .and_then(|id| id.parse().ok())
                .or_else(|| item["id"].as_i64())
        })
        .collect();
    let workitems: BTreeMap<String, Value> = if ids.is_empty() {
        BTreeMap::new()
    } else {
        work_items::rows(ctx, &ado, &ids)?
            .iter()
            .map(WorkItemRef::from)
            .filter_map(|item| {
                let item = serde_json::to_value(item).ok()?;
                let id = item["id"].to_string();
                Some((id, item))
            })
            .collect()
    };
    // The code between the two commits is the next question.
    let built = |build: &Value| {
        Some((
            text(&build["repository"]["name"])?,
            text(&build["sourceVersion"])?,
        ))
    };
    if let (Some((repo, old)), Some((same, new))) = (built(&left_build), built(&right_build))
        && repo == same
        && old != new
    {
        let (base, head) = if left < right { (old, new) } else { (new, old) };
        ctx.note(format!(
            "[next: agent-cli ado diff get {repo}@{base}..{head} --names-only]"
        ));
    }
    let row = |build: &Value| serde_json::to_value(run_row(build));
    let (mut left_row, mut right_row) = (row(&left_build)?, row(&right_build)?);
    // What lies between belongs to the newer run.
    let newer = if right > left {
        &mut right_row
    } else {
        &mut left_row
    };
    newer["commits"] = json!(commits);
    newer["pull_requests"] = json!(prs);
    newer["workitems"] = json!(workitems);
    diff(
        ctx,
        &args.diff,
        &RULES,
        Side::new(left.to_string(), left_row),
        Side::new(right.to_string(), right_row),
    )
}

/// The pull request an Azure Repos merge commit names, and its title or
/// branches: the web's `Merged PR 431: Retry on 429`, or the API's and older
/// servers' `Merge pull request 431 from feature/retry into main`.
fn merged(first: &str) -> Option<(&str, &str)> {
    if let Some(rest) = first.strip_prefix("Merged PR ") {
        return rest.split_once(": ");
    }
    let (number, branches) = first
        .strip_prefix("Merge pull request ")?
        .split_once(" from ")?;
    number
        .chars()
        .all(|c| c.is_ascii_digit())
        .then_some((number, branches))
}

command! {
    pub RUN_DIFF = ["ado", "run", "diff"], Read,
    "Compare two runs of a pipeline: the commits, pull requests, work items it adds",
    keywords: ["deploy", "release", "shipped", "included", "changelog", "builds"],
    example: "ado run diff 8790 8812",
    run: run_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, urls};

    fn build(id: i64, branch: &str, sha: &str) -> Answer {
        Answer::json(
            &json!({"id": id, "buildNumber": format!("2026091{id}"), "status": "completed",
            "result": "succeeded", "sourceBranch": branch, "sourceVersion": sha,
            "definition": {"id": 12, "name": "api-ci"}, "reason": "manual",
            "requestedFor": {"displayName": "Jane Doe"}}),
        )
    }

    #[test]
    fn a_merge_commit_names_its_pull_request_in_either_wording() {
        assert_eq!(
            super::merged("Merged PR 431: Retry on 429"),
            Some(("431", "Retry on 429"))
        );
        assert_eq!(
            super::merged("Merge pull request 36 from feature/retry into main"),
            Some(("36", "feature/retry into main"))
        );
        assert_eq!(super::merged("Merge branch 'main' into feature"), None);
    }

    #[test]
    fn a_run_against_itself_is_the_same_and_reads_nothing_between() {
        let (outcome, transport) = ado(
            &["ado", "run", "diff", "8790", "8790"],
            vec![
                build(8790, "refs/tags/v1.4.1", "2d8b6f4a"),
                build(8790, "refs/tags/v1.4.1", "2d8b6f4a"),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["same"], true, "{outcome:?}");
        assert_eq!(urls(&transport).len(), 2);
    }

    #[test]
    fn two_runs_differ_by_their_commit_and_what_lies_between() {
        let (outcome, transport) = ado(
            &["ado", "run", "diff", "8790", "8812"],
            vec![
                build(8790, "refs/tags/v1.4.1", "2d8b6f4a"),
                build(8812, "refs/tags/v1.4.2", "4be1c0d2"),
                Answer::json(&json!({"count": 2, "value": [
                    {"id": "4be1c0d2", "message": "Merged PR 431: Retry on 429\n\nRelated work items: #1207", "author": {"displayName": "Jane Doe"}},
                    {"id": "9d31e0a7", "message": "Retry the orders call on 429", "author": {"displayName": "Jane Doe"}},
                ]})),
                Answer::json(&json!({"count": 1, "value": [{"id": "1207", "url": "x"}]})),
                Answer::json(&json!({"count": 1, "value": [{"id": 1207, "fields": {
                    "System.WorkItemType": "Bug", "System.Title": "Retry on 429", "System.State": "Resolved"}}]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["changes"],
            json!([
                {"path": "branch", "left": "refs/tags/v1.4.1", "right": "refs/tags/v1.4.2"},
                {"path": "commit", "left": "2d8b6f4a", "right": "4be1c0d2"},
                {"path": "commits", "right": {"4be1c0d2": "Merged PR 431: Retry on 429 (Jane Doe)",
                    "9d31e0a7": "Retry the orders call on 429 (Jane Doe)"}},
                {"path": "pull_requests", "right": {"431": "Retry on 429"}},
                {"path": "workitems", "right": {"1207": {"id": 1207, "type": "Bug",
                    "title": "Retry on 429", "state": "Resolved"}}},
            ])
        );
        assert_eq!(
            urls(&transport)[2],
            format!(
                "{CODE}/build/changes?fromBuildId=8790&toBuildId=8812&$top=200&api-version=7.1-preview.2"
            )
        );
    }
}
