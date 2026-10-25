//! What the crate's tests share: a config with a token, runs of the domain
//! over it, and the pages and people Confluence answers with.

use agent_cli_core::Setup;
use agent_cli_core::testing::{Answer, FakeTransport, Outcome, run};
use serde_json::{Value, json};

pub(crate) const CONFIG: &str = "[confluence]\nurl = \"https://wiki.contoso.example/confluence\"\ntoken_env = \"CONFLUENCE_TEST_TOKEN\"\n";
pub(crate) const TOKEN: &str = "NjM0fixture-pat-5e8c21";
pub(crate) const BASE: &str = "https://wiki.contoso.example/confluence";
pub(crate) const API: &str = "https://wiki.contoso.example/confluence/rest/api";
/// The user keys Data Center gives people.
pub(crate) const JANE: &str = "8a8080a1000000000000000000000001";
pub(crate) const SAM: &str = "8a8080a1000000000000000000000002";

/// Runs `argv` over `answers` with [`CONFIG`] and a token in the
/// environment; the transport shows what was sent.
pub(crate) fn confluence(argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
    confluence_with(Setup::fake(FakeTransport::default()), argv, answers)
}

/// The same with stdin piped in.
pub(crate) fn piped(argv: &[&str], stdin: &str, answers: Vec<Answer>) -> (Outcome, FakeTransport) {
    confluence_with(
        Setup::fake(FakeTransport::default()).with_stdin(stdin),
        argv,
        answers,
    )
}

fn confluence_with(setup: Setup, argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
    let transport = FakeTransport::answering(answers);
    let mut setup = setup
        .with_config(CONFIG)
        .with_env("CONFLUENCE_TEST_TOKEN", TOKEN);
    setup.transport = Box::new(transport.clone());
    let outcome = run(&[crate::DOMAIN], argv, setup);
    assert!(
        !outcome.stdout.contains(TOKEN) && !outcome.stderr.contains(TOKEN),
        "the token leaked: {outcome:?}"
    );
    (outcome, transport)
}

/// `argv --dry-run`: the reads run over `answers`, the first change is
/// planned and nothing that writes is sent. Returns the plans.
pub(crate) fn dry_run(argv: &[&str], answers: Vec<Answer>) -> Vec<Value> {
    let mut argv = argv.to_vec();
    argv.push("--dry-run");
    let (outcome, transport) = confluence(&argv, answers);
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
        "no change was planned: {outcome:?}"
    );
    printed["would"].as_array().cloned().unwrap_or_default()
}

pub(crate) fn urls(transport: &FakeTransport) -> Vec<String> {
    transport.sent().into_iter().map(|sent| sent.url).collect()
}

/// A user as v1 embeds one (`version.by`, `history.createdBy`).
pub(crate) fn user(key: &str, username: &str, name: &str) -> Value {
    json!({"type": "known", "username": username, "userKey": key, "displayName": name})
}

/// A page as `GET /content/{id}?expand=body.storage,version,space,ancestors,metadata.labels,history`
/// answers it: created by Sam Lee, this version by Jane Doe, under 1100.
pub(crate) fn page(id: &str, title: &str, version: u64, storage: &str) -> Value {
    json!({
        "id": id, "type": "page", "status": "current", "title": title,
        "space": {"id": 2001, "key": "ENG", "name": "Engineering", "type": "global"},
        "history": {"latest": true, "createdBy": user(SAM, "sam", "Sam Lee"),
            "createdDate": "2026-09-01T09:00:00.000Z"},
        "version": {"by": user(JANE, "jane", "Jane Doe"), "when": "2026-09-28T16:20:00.000Z",
            "message": "", "number": version, "minorEdit": false},
        "ancestors": [{"id": "1000", "type": "page", "title": "Engineering"},
            {"id": "1100", "type": "page", "title": "Runbooks"}],
        "body": {"storage": {"value": storage, "representation": "storage"}},
        "metadata": {"labels": {"results": [{"prefix": "global", "name": "runbook", "id": "3101"}],
            "start": 0, "limit": 200, "size": 1}},
        "_links": {"webui": format!("/spaces/ENG/pages/{id}/Page"), "tinyui": "/x/TQQ",
            "base": BASE, "context": "/confluence"}
    })
}

pub(crate) fn space() -> Value {
    json!({"id": 2001, "key": "ENG", "name": "Engineering", "type": "global", "status": "current",
        "homepage": {"id": "1000", "type": "page", "title": "Engineering"},
        "_links": {"webui": "/spaces/ENG/overview", "base": BASE, "context": "/confluence"}})
}
