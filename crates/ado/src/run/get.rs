use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Ado, Kind, list, text};
use crate::work_items::{self, WorkItemRef};

use super::{RunIdArgs, RunRow, build_url, is, log_id, run_row, timeline};

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
    Ok(work_items::rows(ctx, ado, &ids)?
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

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, build, page, timeline, urls};

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
}
