use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Ado, Kind, PREVIEW_API, list, query_value, stamp, text};
use crate::thread::{fetch_threads, is_discussion};

use super::{PrRow, fetch_pr, pr_home, pr_row, pr_work_items};

#[derive(clap::Args)]
pub struct PrGetArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// How many of the latest open threads to include
    #[arg(long, default_value_t = 5)]
    comments: usize,
}

/// One pull request with who voted what, what it closes, what its policies
/// say and the latest open discussion.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PullRequest {
    #[serde(flatten)]
    row: PrRow,
    /// Markdown.
    description: Option<String>,
    work_items: Vec<i64>,
    /// Branch policies: builds, reviewer counts, linked work items …
    policies: Vec<Policy>,
    /// Threads still active or pending.
    open_threads: usize,
    /// The latest open threads first.
    threads: Vec<Thread>,
    /// The source commit the merge would take.
    last_merge_source_commit: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Policy {
    name: Option<String>,
    /// approved, rejected, running, queued …
    status: Option<String>,
    /// The build it ran, for a build policy.
    run_id: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Thread {
    /// PR/THREAD: what thread comment and thread update take.
    id: String,
    status: Option<String>,
    author: Option<String>,
    /// The file get id of the lines a code comment is on, at the head.
    at: Option<String>,
    /// The file and line a code comment is on.
    file: Option<String>,
    line: Option<usize>,
    date: Option<String>,
    /// The first comment, Markdown.
    text: Option<String>,
    replies: usize,
}

fn pr_get(ctx: &Ctx, args: PrGetArgs) -> Result<PullRequest> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let pr = fetch_pr(ctx, &ado, id)?;
    let (repo_id, project_id) = pr_home(&pr)?;
    let work_items = pr_work_items(ctx, &ado, &repo_id, id)?;
    let evaluations = ado.get(
        ctx,
        &ado.api(
            Some(&ado.code_project),
            "policy/evaluations",
            &format!(
                "artifactId={}",
                query_value(&format!(
                    "vstfs:///CodeReview/CodeReviewId/{project_id}/{}",
                    id
                ))
            ),
            PREVIEW_API,
        ),
    )?;
    let policies = list(&evaluations["value"])
        .iter()
        .filter(|evaluation| evaluation["status"].as_str() != Some("notApplicable"))
        .map(|evaluation| {
            let configuration = &evaluation["configuration"];
            Policy {
                name: text(&configuration["settings"]["displayName"])
                    .or_else(|| text(&configuration["type"]["displayName"])),
                status: text(&evaluation["status"]),
                run_id: evaluation["context"]["buildId"].as_i64(),
            }
        })
        .collect();
    let threads = fetch_threads(ctx, &ado, &pr, id)?;
    let mut open: Vec<&Value> = threads
        .all
        .iter()
        .filter(|thread| {
            matches!(thread["status"].as_str(), Some("active" | "pending")) && is_discussion(thread)
        })
        .collect();
    open.sort_by(|a, b| {
        b["lastUpdatedDate"]
            .as_str()
            .cmp(&a["lastUpdatedDate"].as_str())
    });
    let open_threads = open.len();
    let threads = open
        .into_iter()
        .take(args.comments)
        .map(|thread| {
            let first = &thread["comments"][0];
            let place = threads.place(thread);
            Thread {
                id: format!("{id}/{}", thread["id"].as_i64().unwrap_or_default()),
                status: text(&thread["status"]),
                author: text(&first["author"]["displayName"]),
                at: place.as_ref().and_then(|place| place.at.clone()),
                line: place.as_ref().and_then(|place| place.line),
                file: place.map(|place| place.file),
                date: stamp(&thread["lastUpdatedDate"]),
                text: text(&first["content"]),
                replies: list(&thread["comments"]).len().saturating_sub(1),
            }
        })
        .collect();
    Ok(PullRequest {
        row: pr_row(&ado, &pr),
        description: text(&pr["description"]),
        work_items,
        policies,
        open_threads,
        threads,
        last_merge_source_commit: text(&pr["lastMergeSourceCommit"]["commitId"]),
    })
}

command! {
    pub PR_GET = ["ado", "pr", "get"], Read,
    "Show a pull request: reviewers and votes, work items, policies, threads",
    keywords: ["review", "approved", "who", "votes", "checks", "status", "comments", "details"],
    example: "ado pr get 42 --fields title,status,reviewers,policies",
    run: pr_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, page, pr, urls};

    #[test]
    fn pr_get_gathers_votes_work_items_policies_and_the_open_threads() {
        let (outcome, transport) = ado(
            &["ado", "pr", "get", "17", "--comments", "1"],
            vec![
                Answer::json(&pr(17, false)),
                page(vec![json!({"id": "42", "url": "x"})]),
                page(vec![
                    json!({"status": "approved", "configuration": {"type": {"displayName": "Minimum number of reviewers"}, "settings": {}}}),
                    json!({"status": "rejected", "configuration": {"type": {"displayName": "Build"},
                        "settings": {"displayName": "web-ci"}}, "context": {"buildId": 991}}),
                    json!({"status": "notApplicable", "configuration": {"type": {"displayName": "Comment requirements"}}}),
                ]),
                page(vec![
                    json!({"id": 1, "sourceRefCommit": {"commitId": "abc123"}}),
                ]),
                page(vec![
                    json!({"id": 1, "status": "active", "lastUpdatedDate": "2026-09-27T00:00:00Z",
                        "comments": [{"author": {"displayName": "Sam Lee"}, "content": "Older", "commentType": "text"}]}),
                    json!({"id": 2, "status": "active", "lastUpdatedDate": "2026-09-28T00:00:00Z",
                        "threadContext": {"filePath": "/src/login.ts", "rightFileStart": {"line": 12}},
                        "comments": [{"author": {"displayName": "Sam Lee"}, "content": "Null check?", "commentType": "text"},
                                     {"author": {"displayName": "Jane Doe"}, "content": "Done", "commentType": "text"}]}),
                    json!({"id": 3, "status": "fixed", "comments": [{"content": "Resolved", "commentType": "text"}]}),
                    json!({"id": 4, "status": "active", "comments": [{"content": "Jane voted 10", "commentType": "system"}]}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["description"], "Fixes **login**");
        assert_eq!(got["work_items"], json!([42]));
        assert_eq!(
            got["policies"],
            json!([{"name": "Minimum number of reviewers", "status": "approved"},
                   {"name": "web-ci", "status": "rejected", "run_id": 991}])
        );
        assert_eq!(got["open_threads"], 2);
        assert_eq!(
            got["threads"],
            json!([{"id": "17/2", "status": "active", "author": "Sam Lee", "at": "web@abc123:src/login.ts:12",
                "file": "src/login.ts", "line": 12,
                "date": "2026-09-28T00:00:00Z", "text": "Null check?", "replies": 1}])
        );
        assert_eq!(got["last_merge_source_commit"], "abc123");
        let sent = urls(&transport);
        assert_eq!(
            sent[0],
            format!("{CODE}/git/pullrequests/17?api-version=7.1")
        );
        assert_eq!(
            sent[2],
            format!(
                "{CODE}/policy/evaluations?artifactId=vstfs%3A%2F%2F%2FCodeReview%2FCodeReviewId%2Fp-1%2F17&api-version=7.1-preview.1"
            )
        );
        assert_eq!(
            sent[4],
            format!(
                "{CODE}/git/repositories/r-1/pullRequests/17/threads?$iteration=1&api-version=7.1"
            ),
            "thread lines are placed on the latest iteration"
        );

        let (outcome, _) = ado(
            &["ado", "pr", "get", "9"],
            vec![Answer::status(
                404,
                r#"{"message":"TF401180: The requested pull request was not found."}"#,
            )],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
