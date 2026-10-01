//! What the contoso world answers of dd. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::ok;

#[test]
fn the_world_answers_what_a_trial_asks_of_dd() {
    for args in [
        &["dd", "monitor", "list"][..],
        &["dd", "monitor", "get", "4712"],
        &["dd", "downtime", "list"],
        &[
            "dd",
            "log",
            "list",
            "--service",
            "worker",
            "--status",
            "error",
        ],
        &[
            "dd",
            "log",
            "list",
            "--pod",
            "prod/web/worker-5c4d3e9f1-q8zt1",
        ],
        &[
            "dd",
            "log",
            "list",
            "--pod",
            "prod/web/worker-5c4d3e9f1-q8zt1",
            "--status",
            "error",
            "--since",
            "1d",
        ],
        &["dd", "log-count", "list", "--status", "error"],
        &[
            "dd",
            "log-count",
            "list",
            "--status",
            "error",
            "--since",
            "1d",
        ],
        &["dd", "event", "list"],
        &["dd", "event", "list", "--since", "1d"],
        &[
            "dd",
            "metric",
            "get",
            "sum:trace.http.request.errors{service:api,env:prod}.as_count()",
            "--since",
            "1d",
        ],
        &["dd", "metric", "list", "trace"],
        &["dd", "service", "list"],
        &["dd", "service", "get", "api"],
        &[
            "dd",
            "service",
            "get",
            "api",
            "--since",
            "2026-09-28T21:30:00Z",
            "--until",
            "2026-09-28T22:30:00Z",
        ],
        &[
            "dd",
            "span",
            "list",
            "--service",
            "api",
            "--status",
            "error",
            "--since",
            "2026-09-28T21:30:00Z",
            "--until",
            "2026-09-28T22:30:00Z",
        ],
        &["dd", "incident", "list"],
        &["dd", "container", "list", "--namespace", "web"],
        &["dd", "host", "list"],
        &["dd", "dashboard", "list", "kubernetes"],
        &[
            "dd",
            "event",
            "list",
            "--since",
            "2026-09-28T21:00:00Z",
            "--until",
            "2026-09-28T23:00:00Z",
        ],
        &["doctor", "dd"],
    ] {
        ok(args);
    }
}

#[test]
fn an_api_exception_names_its_line_in_the_repository_at_the_deployed_commit() {
    let window = [
        "--since",
        "2026-09-28T21:30:00Z",
        "--until",
        "2026-09-28T22:30:00Z",
    ];
    let at =
        "Fabrikam/api@4be1c0d2e8f1a9b3c5d7e9f1a2b3c4d5e6f7a8b9:/app/src/Orders/OrderClient.cs:19";
    let mut args = vec![
        "dd", "log", "list", "--status", "error", "--fields", "id,at",
    ];
    args.extend(window);
    let logs = ok(&args);
    let api = logs
        .as_array()
        .unwrap()
        .iter()
        .find(|log| log["id"] == "AQAAAZIxdapi0004AAAAAEFaSXhkV")
        .unwrap();
    assert_eq!(api["at"], at);
    assert_eq!(logs[0].get("at"), None, "the worker's log has no stack");

    let mut args = vec![
        "dd",
        "span",
        "list",
        "--service",
        "api",
        "--status",
        "error",
        "--fields",
        "trace_id,at",
    ];
    args.extend(window);
    let spans = ok(&args);
    assert_eq!(
        spans[0],
        json!({"trace_id": "1846223019175527734", "at": at}),
        "the span of the same request"
    );
}

#[test]
fn api_since_its_rollout_shows_the_failed_posts_and_the_healthy_traffic() {
    let service = ok(&[
        "dd",
        "service",
        "get",
        "api",
        "--since",
        "2026-09-28T21:34:40Z",
    ]);
    assert_eq!(service["since"], "2026-09-28T21:34:40Z");
    assert_eq!(service["errors"], 212);
    assert_eq!(service["resources"][1]["resource"], "POST /orders");
}

#[test]
fn a_crash_loop_alert_leads_by_notes_alone_to_the_refused_password() {
    let walked = world::follow(&["dd", "monitor", "get", "4712"]);
    assert_eq!(walked.len(), 2, "monitor get, then the log list it names");
    assert!(
        walked[0].stderr.contains(
            "[next: agent-cli dd log list --pod prod/web/worker-5c4d3e9f1-q8zt1 --status error --since 2026-09-28T21:26:00Z]"
        ),
        "{}",
        walked[0].stderr
    );
    let logs = walked[1].json();
    assert_eq!(logs[0]["pod"], "prod/web/worker-5c4d3e9f1-q8zt1");
    assert!(
        logs.as_array().unwrap().iter().all(|log| log["message"]
            .as_str()
            .unwrap()
            .contains("password authentication failed for user \"worker\"")),
        "{logs}"
    );
}
