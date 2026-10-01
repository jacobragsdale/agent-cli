//! What the contoso world answers of k8s. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::{agent_cli, follow, ok};

#[test]
fn the_world_answers_what_a_trial_asks_of_k8s() {
    for args in [
        &["k8s", "pod", "list"][..],
        &["k8s", "pod", "get", "prod/web/worker-5c4d3e9f1-q8zt1"],
        &[
            "k8s",
            "pod",
            "logs",
            "prod/web/worker-5c4d3e9f1-q8zt1",
            "--previous",
        ],
        &["k8s", "event", "list"],
        &["k8s", "configmap", "get", "prod/web/api-config"],
        &["k8s", "secret", "list"],
    ] {
        ok(args);
    }
    let refs = ok(&[
        "k8s",
        "pod",
        "get",
        "prod/web/api-7d9f8c6b5-x2k4q",
        "--fields",
        "secret_refs",
    ]);
    assert_eq!(
        refs["secret_refs"][1]["kv"],
        json!(["kv-contoso-prod/db-password", "kv-contoso-prod/api-key"])
    );
}

#[test]
fn the_crash_looping_pod_leads_by_its_note_to_the_log_saying_the_password_was_refused() {
    let walked = follow(&["k8s", "pod", "get", "prod/web/worker-5c4d3e9f1-q8zt1"]);
    assert_eq!(walked.len(), 2, "pod get, then its previous log");
    assert!(
        walked[0].stderr.contains(
            "[next: agent-cli k8s pod logs prod/web/worker-5c4d3e9f1-q8zt1 --previous --tail 50]"
        ),
        "{}",
        walked[0].stderr
    );
    let log = walked[1].json();
    assert!(
        log["text"]
            .as_str()
            .unwrap()
            .contains("password authentication failed for user \"worker\""),
        "{log}"
    );
}

#[test]
fn the_expired_key_vault_secret_finds_the_worker_pod_that_mounts_it() {
    let pods = ok(&[
        "k8s",
        "pod",
        "list",
        "--kv",
        "kv-contoso-prod/worker-db-password",
        "--fields",
        "id,owner",
    ]);
    assert_eq!(
        pods,
        json!([{"id": "prod/web/worker-5c4d3e9f1-q8zt1", "owner": "Deployment/worker"}])
    );
}

#[test]
fn the_deployments_running_an_image_are_found_by_repo_and_tag() {
    let running = ok(&[
        "k8s",
        "deployment",
        "list",
        "--image",
        "api:v1.4.2",
        "--fields",
        "id",
    ]);
    assert_eq!(running, json!([{"id": "prod/web/api"}]));
}

#[test]
fn a_finished_rollout_names_the_service_to_check_since_it_and_a_failed_one_exits_1() {
    let api = agent_cli(&["k8s", "deployment", "wait", "prod/web/api"]);
    assert_eq!(api.code, 0, "{}{}", api.stdout, api.stderr);
    assert_eq!(api.json()["rolled_out"], "2026-09-28T21:34:40Z");
    assert!(
        api.stderr
            .contains("[next: agent-cli dd service get api --since 2026-09-28T21:34:40Z]"),
        "{}",
        api.stderr
    );

    let worker = agent_cli(&["k8s", "deployment", "wait", "prod/web/worker"]);
    assert_eq!(worker.code, 1, "{}{}", worker.stdout, worker.stderr);
    assert_eq!(worker.json()["ready"], "0/1", "the row is printed as data");
    assert!(
        worker.stderr.contains(
            "[next: agent-cli k8s pod logs prod/web/worker-5c4d3e9f1-q8zt1 --previous --tail 50]"
        ),
        "{}",
        worker.stderr
    );
}
