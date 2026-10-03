//! What the crate's tests share: runs of the domain over one org and
//! project, and the work items, pull requests and builds Azure DevOps
//! answers with.

use agent_cli_core::Setup;
use agent_cli_core::testing::{Answer, FakeTransport, Outcome, run};
use serde_json::{Value, json};

pub(crate) const CONFIG: &str =
    "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\nteam = \"Web Team\"\n";

/// Runs `argv` over `answers` with [`CONFIG`]; the transport shows what
/// was sent.
pub(crate) fn ado(argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
    ado_with(CONFIG, argv, answers)
}

pub(crate) fn ado_with(
    config: &str,
    argv: &[&str],
    answers: Vec<Answer>,
) -> (Outcome, FakeTransport) {
    let transport = FakeTransport::answering(answers);
    let setup = Setup::fake(transport.clone()).with_config(config);
    (run(&[crate::DOMAIN], argv, setup), transport)
}

/// [`ado`] with `stdin` piped in.
pub(crate) fn ado_piped(
    stdin: &str,
    argv: &[&str],
    answers: Vec<Answer>,
) -> (Outcome, FakeTransport) {
    let transport = FakeTransport::answering(answers);
    let setup = Setup::fake(transport.clone())
        .with_config(CONFIG)
        .with_stdin(stdin);
    (run(&[crate::DOMAIN], argv, setup), transport)
}

/// The URLs sent, in order.
pub(crate) fn urls(transport: &FakeTransport) -> Vec<String> {
    transport.sent().into_iter().map(|sent| sent.url).collect()
}

/// `testing::assert_dry_run` for commands that read `[ado]` first: the
/// reads run, the first change is planned and never sent.
pub(crate) fn dry_run(argv: &[&str], answers: Vec<Answer>) -> Vec<Value> {
    let mut argv = argv.to_vec();
    argv.push("--dry-run");
    let (outcome, transport) = ado(&argv, answers);
    let writes: Vec<_> = transport
        .sent()
        .into_iter()
        .filter(|sent| !sent.method.is_read())
        .collect();
    assert!(
        writes.is_empty(),
        "a change was sent under --dry-run: {writes:?}"
    );
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let printed = outcome.json();
    assert_eq!(
        printed["dry_run"], true,
        "never reached ctx.write: {outcome:?}"
    );
    assert_eq!(transport.remaining(), 0, "answers left over: {outcome:?}");
    printed["would"].as_array().cloned().unwrap_or_default()
}

pub(crate) const BASE: &str = "https://dev.azure.com/contoso";

pub(crate) const CODE: &str = "https://dev.azure.com/contoso/Fabrikam/_apis";

pub(crate) fn page(items: Vec<Value>) -> Answer {
    Answer::json(&json!({"count": items.len(), "value": items}))
}

pub(crate) fn item(id: i64, rev: i64, title: &str) -> Value {
    json!({"id": id, "rev": rev, "fields": {
        "System.Id": id,
        "System.WorkItemType": "Bug",
        "System.Title": title,
        "System.State": "Active",
        "System.AssignedTo": {"displayName": "Jane Doe", "uniqueName": "jane@contoso.com"},
        "System.IterationPath": "Fabrikam\\Sprint 12",
        "System.AreaPath": "Fabrikam\\Web",
        "Microsoft.VSTS.Common.Priority": 2,
        "System.Tags": "ui; p1",
        "System.ChangedDate": "2026-09-28T10:00:00.000Z"
    }})
}

pub(crate) fn wiql(ids: &[i64]) -> Answer {
    let items: Vec<Value> = ids.iter().map(|id| json!({"id": id, "url": "x"})).collect();
    Answer::json(&json!({"queryType": "flat", "workItems": items}))
}

pub(crate) fn batch(items: Vec<Value>) -> Answer {
    Answer::json(&json!({"count": items.len(), "value": items}))
}

/// What `git/repositories/web` answers.
pub(crate) fn repo() -> Answer {
    Answer::json(&json!({"id": "r-1", "name": "web",
        "defaultBranch": "refs/heads/main", "project": {"id": "p-1", "name": "Fabrikam"}}))
}

pub(crate) fn repos() -> Answer {
    Answer::json(&json!({"count": 1, "value": [{"id": "r-1", "name": "web",
        "defaultBranch": "refs/heads/main", "project": {"id": "p-1", "name": "Fabrikam"}}]}))
}

pub(crate) fn pr(id: i64, draft: bool) -> Value {
    json!({
        "pullRequestId": id,
        "repository": {"id": "r-1", "name": "web", "project": {"id": "p-1", "name": "Fabrikam"}},
        "title": format!("Change {id}"),
        "status": "active",
        "isDraft": draft,
        "createdBy": {"displayName": "Jane Doe", "uniqueName": "jane@contoso.com"},
        "creationDate": "2026-09-28T10:00:00Z",
        "sourceRefName": "refs/heads/42-fix-login",
        "targetRefName": "refs/heads/main",
        "mergeStatus": "succeeded",
        "lastMergeSourceCommit": {"commitId": "abc123"},
        "description": "Fixes **login**",
        "reviewers": [
            {"id": "u-2", "displayName": "Sam Lee", "vote": 10, "isRequired": true},
            {"id": "u-3", "displayName": "Web Team", "vote": -5}
        ]
    })
}

pub(crate) fn me() -> Answer {
    Answer::json(&json!({"authenticatedUser": {"id": "u-1", "providerDisplayName": "Jane Doe"}}))
}

pub(crate) fn build(id: i64, status: &str, result: Option<&str>) -> Value {
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

pub(crate) fn record(
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

pub(crate) fn timeline() -> Answer {
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

/// The identity search finding one person.
pub(crate) fn person(id: &str, name: &str, email: &str) -> Answer {
    Answer::json(
        &json!({"count": 1, "value": [{"id": id, "providerDisplayName": name,
        "properties": {"Mail": {"$value": email}, "Account": {"$value": email}}}]}),
    )
}

/// What `types::fields` reads for a User Story: its fields, then the
/// project's data types.
pub(crate) fn story_fields() -> Vec<Answer> {
    vec![
        page(vec![
            json!({"name": "Title", "referenceName": "System.Title", "alwaysRequired": true}),
            json!({"name": "State", "referenceName": "System.State", "alwaysRequired": true}),
            json!({"name": "Story Points", "referenceName": "Microsoft.VSTS.Scheduling.StoryPoints"}),
            json!({"name": "Risk", "referenceName": "Microsoft.VSTS.Common.Risk",
                "allowedValues": ["1 - High", "2 - Medium", "3 - Low"]}),
            json!({"name": "Repro Steps", "referenceName": "Microsoft.VSTS.TCM.ReproSteps"}),
        ]),
        page(vec![
            json!({"referenceName": "System.Title", "type": "string"}),
            json!({"referenceName": "System.State", "type": "string"}),
            json!({"referenceName": "Microsoft.VSTS.Scheduling.StoryPoints", "type": "double"}),
            json!({"referenceName": "Microsoft.VSTS.Common.Risk", "type": "string"}),
            json!({"referenceName": "Microsoft.VSTS.TCM.ReproSteps", "type": "html"}),
        ]),
    ]
}
