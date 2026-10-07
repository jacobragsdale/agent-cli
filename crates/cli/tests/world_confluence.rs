//! What the contoso world answers of confluence. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::{agent_cli, ok};

#[test]
fn the_world_answers_what_a_trial_asks_of_confluence() {
    for args in [
        &["doctor", "confluence"][..],
        &["confluence", "space", "list"],
        &["confluence", "page", "list", "runbook etl_nightly"],
        &["confluence", "page", "list", "orders search indexer"],
        &["confluence", "page", "list", "--label", "runbook"],
        &["confluence", "page", "list", "--since", "7d"],
        &["confluence", "page", "get", "1101"],
        &["confluence", "page", "get", "1102", "1201"],
        &["confluence", "page", "get", "1300"],
        &["confluence", "page", "get", "1201@2"],
        &[
            "confluence",
            "page",
            "get",
            "https://contoso.atlassian.net/wiki/spaces/ENG/pages/1101/Runbook+etl_nightly",
        ],
        &["confluence", "tree", "get", "ENG"],
        &["confluence", "tree", "get", "1100"],
        &["confluence", "comment", "list", "1101"],
        &["confluence", "comment", "list", "1201"],
        &["confluence", "attachment", "list", "1201"],
        &["confluence", "attachment", "get", "att7001"],
        &["confluence", "version", "list", "1201"],
        &["confluence", "version", "list", "1101"],
    ] {
        ok(args);
    }
}

#[test]
fn the_runbook_section_names_the_fix_and_its_open_comment() {
    let section = ok(&[
        "confluence",
        "page",
        "get",
        "ENG:Runbook: etl_nightly",
        "--section",
        "an order without customer_id",
    ]);
    let body = section["body"].as_str().unwrap();
    for step in [
        "Fix the order in the CRM",
        "retry the load",
        "`orders-sql` on `srch-contoso-prod`",
    ] {
        assert!(body.contains(step), "{step}: {body}");
    }
    assert_eq!(section["lossy"], json!(["inline comment marks"]));
    let open = ok(&["confluence", "comment", "list", "1101", "--open"]);
    assert_eq!(open[0]["selection"], "retry the load");
    assert_eq!(open[0]["author"], "Sam Lee");
    assert_eq!(open[0]["body"], "only once the CRM fix has synced");
}

#[test]
fn the_release_notes_link_ado_and_their_changelog_downloads() {
    let notes = ok(&[
        "confluence",
        "page",
        "get",
        "1201",
        "--fields",
        "body,updated_by,version",
    ]);
    let body = notes["body"].as_str().unwrap();
    assert!(
        body.contains("https://dev.azure.com/contoso/Fabrikam/_git/api/pullrequest/431"),
        "{body}"
    );
    assert_eq!(notes["updated_by"], "Jane Doe");
    let changes = ok(&["confluence", "attachment", "get", "att7001"]);
    assert!(changes["text"].as_str().unwrap().contains("PR 431"));
    let png = agent_cli(&["confluence", "attachment", "get", "att7002"]);
    assert_eq!(png.code, 2, "a binary file needs --output: {}", png.stderr);
}

#[test]
fn a_rollback_section_appends_to_the_runbook_and_leaves_the_rest() {
    let plan = ok(&[
        "confluence",
        "page",
        "update",
        "1101",
        "--section",
        "Escalation",
        "--append",
        "## Rollback\n\nClear the run's tasks in Airflow.",
        "--dry-run",
    ]);
    let sent = plan["would"][0]["body"]["body"]["value"].as_str().unwrap();
    assert!(
        sent.ends_with(".</p><h2>Rollback</h2><p>Clear the run&rsquo;s tasks in Airflow.</p>")
            || sent.ends_with(".</p><h2>Rollback</h2><p>Clear the run's tasks in Airflow.</p>"),
        "{sent}"
    );
    assert!(sent.contains("<ac:inline-comment-marker ac:ref=\"5d3e2f1a-0000-4000-8000-000000005002\">retry the load</ac:inline-comment-marker>"));
}
