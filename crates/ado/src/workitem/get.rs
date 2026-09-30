use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, COMMENTS_API, Kind, list, stamp, text};
use crate::markdown::html_to_markdown;
use crate::work_items::{Artifact, WorkItemRow, artifact, person, relations, row};

use super::PARENT;

const CHILD: &str = "System.LinkTypes.Hierarchy-Forward";

const RELATED: &str = "System.LinkTypes.Related";

#[derive(clap::Args)]
pub struct GetArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// How many of the latest comments to include
    #[arg(long, default_value_t = 5)]
    comments: usize,
}

/// One work item, its text as Markdown, its links and its latest comments.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkItem {
    #[serde(flatten)]
    row: WorkItemRow,
    parent: Option<i64>,
    children: Vec<i64>,
    related: Vec<i64>,
    pull_requests: Vec<PrLink>,
    branches: Vec<BranchLink>,
    /// Markdown.
    description: Option<String>,
    /// Markdown.
    acceptance_criteria: Option<String>,
    comment_count: Option<i64>,
    /// The latest first.
    comments: Vec<Comment>,
    url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrLink {
    repo: String,
    id: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BranchLink {
    repo: String,
    name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Comment {
    id: i64,
    author: Option<String>,
    date: Option<String>,
    /// Markdown.
    text: String,
}

fn linked_id(url: &str) -> Option<i64> {
    url.rsplit('/').next()?.parse().ok()
}

fn workitem_get(ctx: &Ctx, args: GetArgs) -> Result<WorkItem> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let item = ado.get(
        ctx,
        &ado.api(
            None,
            &format!("wit/workitems/{id}"),
            "$expand=relations",
            crate::client::API,
        ),
    )?;
    let mut work = WorkItem {
        row: row(&item),
        parent: None,
        children: Vec::new(),
        related: Vec::new(),
        pull_requests: Vec::new(),
        branches: Vec::new(),
        description: text(&item["fields"]["System.Description"])
            .map(|html| html_to_markdown(&html)),
        acceptance_criteria: text(&item["fields"]["Microsoft.VSTS.Common.AcceptanceCriteria"])
            .map(|html| html_to_markdown(&html)),
        comment_count: None,
        comments: Vec::new(),
        url: ado.work_item_url(id),
    };
    let mut artifacts = Vec::new();
    for (rel, url) in relations(&item) {
        match rel {
            PARENT => work.parent = linked_id(url),
            CHILD => work.children.extend(linked_id(url)),
            RELATED => work.related.extend(linked_id(url)),
            "ArtifactLink" => artifacts.extend(artifact(url)),
            _ => {}
        }
    }
    if !artifacts.is_empty() {
        let repos = ado.repos(ctx, false)?;
        let name = |repo_id: &str| {
            repos
                .iter()
                .find(|repo| repo.id.eq_ignore_ascii_case(repo_id))
                .map_or_else(|| repo_id.to_owned(), |repo| repo.name.clone())
        };
        for artifact in artifacts {
            match artifact {
                Artifact::PullRequest { repo_id, id } => work.pull_requests.push(PrLink {
                    repo: name(&repo_id),
                    id,
                }),
                Artifact::Branch {
                    repo_id,
                    name: branch,
                } => work.branches.push(BranchLink {
                    repo: name(&repo_id),
                    name: branch,
                }),
            }
        }
    }
    if args.comments > 0 {
        let url = ado.api(
            Some(&ado.project),
            &format!("wit/workItems/{id}/comments"),
            &format!("$top={}&order=desc", args.comments),
            COMMENTS_API,
        );
        let page = ado.get(ctx, &url)?;
        work.comment_count = page["totalCount"].as_i64();
        work.comments = list(&page["comments"])
            .iter()
            .filter_map(|comment| {
                let text = html_to_markdown(comment["text"].as_str().unwrap_or_default());
                (!text.is_empty()).then(|| Comment {
                    id: comment["id"].as_i64().unwrap_or_default(),
                    author: person(&comment["createdBy"]),
                    date: stamp(&comment["createdDate"]),
                    text,
                })
            })
            .collect();
    }
    Ok(work)
}

command! {
    pub WORKITEM_GET = ["ado", "workitem", "get"], Read,
    "Show a work item: fields, description as Markdown, links, latest comments",
    keywords: ["ticket", "details", "description", "acceptance", "criteria", "parent", "children", "read"],
    example: "ado workitem get 42 --fields id,title,state,description",
    run: workitem_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, item, repos, urls};

    #[test]
    fn get_shows_markdown_links_by_repo_name_and_the_latest_comments() {
        let mut work = item(42, 7, "Login fails on Safari");
        work["fields"]["System.Description"] =
            json!("<p>Steps:</p><ol><li>Open <b>login</b></li></ol>");
        work["fields"]["Microsoft.VSTS.Common.AcceptanceCriteria"] =
            json!("<ul><li>works</li></ul>");
        work["relations"] = json!([
            {"rel": "System.LinkTypes.Hierarchy-Reverse", "url": format!("{BASE}/_apis/wit/workItems/7")},
            {"rel": "System.LinkTypes.Hierarchy-Forward", "url": format!("{BASE}/_apis/wit/workItems/43")},
            {"rel": "System.LinkTypes.Hierarchy-Forward", "url": format!("{BASE}/_apis/wit/workItems/44")},
            {"rel": "System.LinkTypes.Related", "url": format!("{BASE}/_apis/wit/workItems/9")},
            {"rel": "ArtifactLink", "url": "vstfs:///Git/PullRequestId/p-1%2Fr-1%2F17"},
            {"rel": "ArtifactLink", "url": "vstfs:///Git/Ref/p-1%2Fr-1%2FGB42-fix%2Fsafari"},
            {"rel": "ArtifactLink", "url": "vstfs:///Build/Build/991"}
        ]);
        let (outcome, transport) = ado(
            &["ado", "workitem", "get", "42", "--comments", "2"],
            vec![
                Answer::json(&work),
                repos(),
                Answer::json(&json!({"totalCount": 12, "count": 2, "comments": [
                    {"id": 501, "text": "<div>Repro on <code>17.2</code></div>",
                     "createdBy": {"displayName": "Sam Lee"}, "createdDate": "2026-09-28T09:00:00Z"},
                    {"id": 500, "text": "<p></p>", "createdBy": {"displayName": "Sam Lee"},
                     "createdDate": "2026-09-27T09:00:00Z"}
                ]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["title"], "Login fails on Safari");
        assert_eq!(got["rev"], 7);
        assert_eq!(got["parent"], 7);
        assert_eq!(got["children"], json!([43, 44]));
        assert_eq!(got["related"], json!([9]));
        assert_eq!(got["pull_requests"], json!([{"repo": "web", "id": 17}]));
        assert_eq!(
            got["branches"],
            json!([{"repo": "web", "name": "42-fix/safari"}])
        );
        assert_eq!(got["description"], "Steps:\n\n1. Open **login**");
        assert_eq!(got["acceptance_criteria"], "- works");
        assert_eq!(got["comment_count"], 12);
        assert_eq!(
            got["comments"],
            json!([{"id": 501, "author": "Sam Lee", "date": "2026-09-28T09:00:00Z", "text": "Repro on `17.2`"}])
        );
        assert_eq!(got["url"], format!("{BASE}/Fabrikam/_workitems/edit/42"));
        assert_eq!(
            urls(&transport),
            [
                format!("{BASE}/_apis/wit/workitems/42?$expand=relations&api-version=7.1"),
                format!("{BASE}/Fabrikam/_apis/git/repositories?api-version=7.1"),
                format!(
                    "{BASE}/Fabrikam/_apis/wit/workItems/42/comments?$top=2&order=desc&api-version=7.1-preview.4"
                ),
            ]
        );
    }

    #[test]
    fn get_of_a_missing_item_is_4_and_a_sign_in_page_is_3() {
        let (outcome, transport) = ado(
            &["ado", "workitem", "get", "41", "--comments", "0"],
            vec![Answer::json(&item(41, 1, "No links"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent().len(),
            1,
            "no comments asked, no links to name"
        );

        let (outcome, _) = ado(
            &["ado", "workitem", "get", "404"],
            vec![Answer::status(
                404,
                r#"{"message":"TF401232: Work item 404 does not exist."}"#,
            )],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(outcome.stderr.contains("TF401232"), "{}", outcome.stderr);

        let (outcome, _) = ado(
            &["ado", "workitem", "get", "42"],
            vec![Answer::status(203, "<html><body>Sign in</body></html>")],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: run `az login`, or set AZURE_DEVOPS_EXT_PAT"),
            "{}",
            outcome.stderr
        );
    }
}
