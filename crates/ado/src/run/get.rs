use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Ado, Kind, list, text};
use crate::ids::arg;
use crate::test::{has_failures, test_runs};
use crate::work_items::{self, WorkItemRef};

use super::{
    RunIdArgs, RunRow, build_url, built, built_tree, is, line_at, log_id, run_row, timeline,
};

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
    errors: Vec<BuildError>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BuildError {
    message: String,
    /// The line it names (a compiler's, a problem matcher's), as file get
    /// takes it.
    at: Option<String>,
}

/// Where a timeline issue says it is: `data.sourcepath` as the agent saw
/// it and `data.linenumber`, which compilers and problem matchers fill.
fn issue_place(issue: &Value) -> Option<(String, usize)> {
    let data = &issue["data"];
    let line = data["linenumber"]
        .as_str()
        .and_then(|line| line.trim().parse().ok())
        .or_else(|| {
            data["linenumber"]
                .as_u64()
                .and_then(|line| line.try_into().ok())
        })
        .filter(|line| *line > 0)?;
    Some((text(&data["sourcepath"])?, line))
}

fn errors(record: &Value) -> impl Iterator<Item = &Value> {
    list(&record["issues"])
        .iter()
        .filter(|issue| issue["type"].as_str() == Some("error"))
}

/// The command that answers what a run's reader asks next: what it waits
/// on, or why it failed (a failing test, the line an error names, else the
/// first failed task's log).
fn next_step(ctx: &Ctx, ado: &Ado, id: i64, detail: &RunDetail) -> Option<String> {
    match (detail.run.status.as_deref(), detail.run.result.as_deref()) {
        (Some("notStarted" | "inProgress"), _) => return Some(format!("ado run wait {id}")),
        (_, Some("failed")) => {}
        _ => return None,
    }
    let runs = test_runs(ctx, ado, id).unwrap_or_else(|error| {
        ctx.note(format!("[test results not read: {error:#}]"));
        Vec::new()
    });
    if runs.iter().any(has_failures) {
        return Some(format!("ado test list {id}"));
    }
    let failed = &detail.failed;
    if let Some(at) = failed
        .iter()
        .flat_map(|step| &step.errors)
        .find_map(|error| error.at.as_deref())
    {
        return Some(format!("ado file get {}", arg(at)));
    }
    let task = failed
        .iter()
        .find(|step| step.kind.as_deref() == Some("Task"))
        .and_then(|step| step.name.as_deref());
    Some(match task {
        Some(task) => format!("ado run logs {id} --task {}", arg(task)),
        None => format!("ado run logs {id}"),
    })
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
    let failed_records = || {
        records
            .iter()
            .filter(|record| record["result"].as_str() == Some("failed"))
    };
    let built = built(&build);
    let placed = failed_records()
        .flat_map(errors)
        .any(|issue| issue_place(issue).is_some());
    let files = built_tree(ctx, &ado, built.as_ref(), placed)?;
    let failed_of = |kind: Option<&str>| -> Vec<FailedStep> {
        failed_records()
            .filter(|record| kind.is_none_or(|kind| is(record, kind)))
            .map(|record| FailedStep {
                kind: text(&record["type"]),
                name: text(&record["name"]),
                log_id: log_id(record),
                errors: errors(record)
                    .filter_map(|issue| {
                        Some(BuildError {
                            message: text(&issue["message"])?,
                            at: built.as_ref().zip(issue_place(issue)).and_then(
                                |(built, (path, line))| line_at(built, &files, &path, line),
                            ),
                        })
                    })
                    .collect(),
            })
            .collect()
    };
    let mut failed = failed_of(Some("Task"));
    if failed.is_empty() {
        failed = failed_of(None);
    }
    let detail = RunDetail {
        run: run_row(&build),
        pr,
        workitems,
        running: records
            .iter()
            .filter(|record| is(record, "Task") && record["state"].as_str() == Some("inProgress"))
            .filter_map(|record| text(&record["name"]))
            .collect(),
        failed,
    };
    if let Some(next) = next_step(ctx, &ado, id, &detail) {
        ctx.note(format!("[next: agent-cli {next}]"));
    }
    Ok(detail)
}

command! {
    pub RUN_GET = ["ado", "run", "get"], Read,
    "Show a run (build): status, commit, what failed, its pull request and work items",
    keywords: ["build", "why", "failed", "broken", "status", "result", "timeline", "timing", "errors", "shipped", "deployed", "release"],
    example: "ado run get 1234 --fields status,result,commit,pr,workitems,failed",
    run: run_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, build, page, record, timeline, urls};

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
                page(vec![]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["pr"], json!({"id": 432}));
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli ado run logs 8813]"),
            "no failed task to name: {}",
            outcome.stderr
        );
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
                page(vec![]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["result"], "failed");
        assert_eq!(got["commit"], "abc123");
        assert_eq!(
            got["failed"],
            json!([{"type": "Task", "name": "Run tests", "log_id": 5,
                "errors": [{"message": "3 tests failed"}]}])
        );
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli ado run logs 991 --task 'Run tests']"),
            "{}",
            outcome.stderr
        );
        let sent = urls(&transport);
        assert_eq!(
            sent[1],
            format!("{CODE}/build/builds/991/timeline?api-version=7.1")
        );
        assert_eq!(
            sent[3],
            format!(
                "{CODE}/test/runs?buildUri=vstfs%3A%2F%2F%2FBuild%2FBuild%2F991&api-version=7.1"
            )
        );

        let (outcome, _) = ado(
            &["ado", "run", "get", "991"],
            vec![
                Answer::json(&build(991, "completed", Some("failed"))),
                timeline(),
                page(vec![]),
                page(vec![json!({"id": 501, "totalTests": 9, "passedTests": 6})]),
            ],
        );
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli ado test list 991]"),
            "failed tests come first: {}",
            outcome.stderr
        );
    }

    #[test]
    fn a_compiler_error_names_its_repository_line_and_that_is_the_next_step() {
        let mut built = build(8814, "completed", Some("failed"));
        built["reason"] = json!("pullRequest");
        built["sourceVersion"] = json!("merge01");
        built["triggerInfo"] = json!({"pr.number": "436", "pr.sourceSha": "head01"});
        built["repository"] = json!({"id": "r-1", "type": "TfsGit", "name": "api"});
        let mut task = record("t2", "Task", "Build", None, Some("failed"), 5);
        task["issues"] = json!([
            {"type": "error", "message": "'Random' does not contain a definition for 'Shared'",
             "data": {"type": "error", "sourcepath": "/home/vsts/work/1/s/src/Orders/OrderClient.cs",
                      "linenumber": "42", "columnnumber": "29", "code": "CS0117"}},
            {"type": "error", "message": "Bash exited with code '1'."}
        ]);
        let (outcome, transport) = ado(
            &["ado", "run", "get", "8814"],
            vec![
                Answer::json(&built),
                Answer::json(&json!({"records": [task]})),
                page(vec![]),
                Answer::json(&json!({"results": [{}]})),
                page(vec![json!({"path": "/src/Orders/OrderClient.cs"})]),
                page(vec![]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["failed"][0]["errors"],
            json!([
                {"message": "'Random' does not contain a definition for 'Shared'",
                 "at": "api@head01:src/Orders/OrderClient.cs:42"},
                {"message": "Bash exited with code '1'."}
            ])
        );
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli ado file get api@head01:src/Orders/OrderClient.cs:42]"),
            "{}",
            outcome.stderr
        );
        assert!(
            urls(&transport)[4].contains("versionDescriptor.version=head01"),
            "the pull request's head, not its merge"
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
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli ado run wait 992]"),
            "{}",
            outcome.stderr
        );

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
