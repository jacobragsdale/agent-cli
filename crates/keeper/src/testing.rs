//! What the crate's tests share: a config with a token and one without,
//! runs of the domain over recorded answers, and the rows Keeper answers.

use agent_cli_core::Setup;
use agent_cli_core::testing::{Answer, FakeTransport, Outcome, run};
use serde_json::{Value, json};

use crate::client::Keeper;

pub(crate) const BASE: &str = "https://keeper.contoso.example";
pub(crate) const API: &str = "https://keeper.contoso.example/api/v1";
pub(crate) const CONFIG: &str =
    "[keeper]\nurl = \"https://keeper.contoso.example\"\ntoken_env = \"KEEPER_TEST_TOKEN\"\n";
pub(crate) const ANONYMOUS: &str = "[keeper]\nurl = \"https://keeper.contoso.example\"\n";
pub(crate) const TOKEN: &str = "a1b2c3d4_fixturetoken0000000000000000";

pub(crate) fn keeper() -> Keeper {
    Keeper::anonymous(BASE)
}

/// Runs `argv` over `answers` with [`CONFIG`] and its token.
pub(crate) fn keeper_run(argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
    with_config(CONFIG, argv, answers)
}

/// The same with no token.
pub(crate) fn anonymous(argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
    with_config(ANONYMOUS, argv, answers)
}

fn with_config(config: &str, argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
    let transport = FakeTransport::answering(answers);
    let setup = Setup::fake(transport.clone())
        .with_config(config)
        .with_env("KEEPER_TEST_TOKEN", TOKEN);
    let outcome = run(&[crate::DOMAIN], argv, setup);
    assert!(
        !outcome.stdout.contains(TOKEN) && !outcome.stderr.contains(TOKEN),
        "the token leaked: {outcome:?}"
    );
    (outcome, transport)
}

pub(crate) fn urls(transport: &FakeTransport) -> Vec<String> {
    transport.sent().into_iter().map(|sent| sent.url).collect()
}

/// A page of a page-numbered list.
pub(crate) fn page(items: Vec<Value>, total: u64) -> Answer {
    let pages = total.div_ceil(100).max(1);
    Answer::json(&json!({"items": items,
        "pagination": {"page": 1, "per_page": 100, "total": total, "total_pages": pages}}))
}

/// A page of a repository's files, keyset-paged.
pub(crate) fn files(items: Vec<Value>, next: Option<&str>) -> Answer {
    let total = items.len();
    Answer::json(&json!({"items": items,
        "pagination": {"page": 1, "per_page": 100, "total": total, "total_pages": 1},
        "next_cursor": next, "has_more": next.is_some()}))
}

/// A repository as `GET /repositories/{key}` answers it (trimmed).
pub(crate) fn repository(key: &str, format: &str, kind: &str) -> Value {
    let upstream = (kind == "remote").then_some("https://pypi.org");
    json!({"id": "00000000-0000-4000-8000-00000000r001", "key": key, "name": key,
        "description": null, "format": format, "repo_type": kind, "visibility": "public",
        "is_public": true, "storage_used_bytes": 1_048_576, "quota_bytes": null,
        "versioning_enabled": false, "upstream_url": upstream, "curation_enabled": false,
        "created_at": "2026-09-01T09:00:00.123456Z", "updated_at": "2026-09-28T21:30:00.5Z"})
}

/// A file as the artifact list and `GET /repositories/{key}/artifacts/{path}` answer it.
pub(crate) fn artifact(repo: &str, path: &str, sha: &str) -> Value {
    let mut parts = path.split('/');
    let (name, version) = match (parts.next(), parts.next(), parts.next()) {
        (Some(name), Some(version), Some(_)) => (json!(name), json!(version)),
        _ => (json!(path.rsplit('/').next()), Value::Null),
    };
    json!({"id": "00000000-0000-4000-8000-00000000a001", "repository_key": repo, "path": path,
        "name": name, "version": version, "size_bytes": 2048, "checksum_sha256": sha,
        "content_type": "application/gzip", "download_count": 3,
        "created_at": "2026-09-28T21:30:05.250000Z", "uploaded_by": "00000000-0000-4000-8000-00000000u001",
        "uploaded_by_username": "svc-pipelines", "metadata": null, "analyzable": true,
        "quarantine_status": "not_quarantined", "origin": {"v": 1, "kind": "hosted", "repository_key": repo},
        "last_promotion": null})
}
