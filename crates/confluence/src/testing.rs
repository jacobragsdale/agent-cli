//! What the crate's tests share: a config with a token, runs of the domain
//! over it, and the pages and people Confluence answers with.

use agent_cli_core::testing::{Answer, FakeTransport, Outcome, run};
use agent_cli_core::{Setup, base64};
use serde_json::{Value, json};

pub(crate) const CONFIG: &str = "[confluence]\nurl = \"https://contoso.atlassian.net/wiki\"\nemail = \"jane@contoso.com\"\ntoken_env = \"CONFLUENCE_TEST_TOKEN\"\n";
pub(crate) const TOKEN: &str = "atl-fixture-token-5e8c21";
pub(crate) const V2: &str = "https://contoso.atlassian.net/wiki/api/v2";
pub(crate) const V1: &str = "https://contoso.atlassian.net/wiki/rest/api";
pub(crate) const SAM: &str = "557058:00000000-0000-4000-8000-00000000a002";
pub(crate) const JANE: &str = "557058:00000000-0000-4000-8000-00000000a001";

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
    let basic = base64(format!("jane@contoso.com:{TOKEN}").as_bytes());
    for leaked in [TOKEN, basic.as_str()] {
        assert!(
            !outcome.stdout.contains(leaked) && !outcome.stderr.contains(leaked),
            "the token leaked: {outcome:?}"
        );
    }
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

/// A v2 page as `GET /pages/{id}?body-format=storage` answers it.
pub(crate) fn page(id: &str, title: &str, version: u64, storage: &str) -> Value {
    json!({
        "id": id, "status": "current", "title": title, "spaceId": "2001", "parentId": "1100",
        "parentType": "page", "authorId": SAM, "createdAt": "2026-09-01T09:00:00.000Z",
        "version": {"number": version, "message": "", "minorEdit": false, "authorId": JANE,
            "createdAt": "2026-09-28T16:20:00.000Z"},
        "body": {"storage": {"representation": "storage", "value": storage}},
        "labels": {"results": [{"prefix": "global", "name": "runbook", "id": "3101"}]},
        "_links": {"webui": format!("/spaces/ENG/pages/{id}/Page"), "tinyui": "/x/TQQ",
            "base": "https://contoso.atlassian.net/wiki"}
    })
}

pub(crate) fn space() -> Value {
    json!({"id": "2001", "key": "ENG", "name": "Engineering", "type": "global", "status": "current",
        "homepageId": "1000", "_links": {"webui": "/spaces/ENG"}})
}

pub(crate) fn spaces() -> Answer {
    Answer::json(&json!({"results": [space()], "_links": {}}))
}

pub(crate) fn people() -> Answer {
    Answer::json(&json!({"results": [
        {"accountId": JANE, "displayName": "Jane Doe"},
        {"accountId": SAM, "displayName": "Sam Lee"}
    ]}))
}
