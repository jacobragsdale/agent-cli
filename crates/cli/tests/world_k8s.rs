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
        &["k8s", "kind", "list"],
        &["k8s", "service", "list"],
        &["k8s", "service", "get", "prod/web/api"],
        &["k8s", "ingress", "list"],
        &["k8s", "networkpolicy", "list"],
        &["k8s", "networkpolicy", "get", "prod/web/allow-egress"],
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
            "hint: agent-cli k8s pod logs prod/web/worker-5c4d3e9f1-q8zt1 --previous --tail 50"
        ),
        "{}",
        worker.stderr
    );
}

#[test]
fn a_custom_resource_and_a_node_read_through_the_generic_floor_by_the_ids_it_lists() {
    let kinds = ok(&[
        "k8s",
        "kind",
        "list",
        "cert",
        "--fields",
        "name,api_version,namespaced",
    ]);
    assert_eq!(
        kinds,
        json!([{"name": "certificates", "api_version": "cert-manager.io/v1", "namespaced": true}])
    );
    let listed = ok(&[
        "k8s",
        "object",
        "list",
        "certificates",
        "--fields",
        "id,age,ready,status",
    ]);
    assert_eq!(
        listed,
        json!([{"id": "prod/web/api-tls", "age": "1y", "ready": true, "status": "Ready"}])
    );
    let got = ok(&["k8s", "object", "get", "cert", "prod/web/api-tls"]);
    let manifest = &got["manifest"];
    assert_eq!(manifest["status"]["notAfter"], "2026-11-30T03:00:00Z");
    assert!(
        manifest["metadata"].get("annotations").is_none()
            && manifest["metadata"].get("managedFields").is_none(),
        "{manifest}"
    );

    let nodes = ok(&["k8s", "object", "list", "nodes", "--fields", "id,ready"]);
    assert_eq!(
        nodes,
        json!([{"id": "prod/aks-system-31415926-vmss000001", "ready": true}])
    );
    let node = ok(&[
        "k8s",
        "object",
        "get",
        "node",
        "prod/aks-system-31415926-vmss000001",
    ]);
    assert!(
        node["manifest"]["spec"]["providerID"]
            .as_str()
            .is_some_and(|id| id.ends_with("/virtualMachines/1")),
        "{node}"
    );
}

#[test]
fn the_worker_service_answers_nothing_and_its_notes_lead_to_the_refused_password() {
    let walked = follow(&["k8s", "service", "get", "prod/web/worker"]);
    assert_eq!(
        walked.len(),
        3,
        "service get, the pod, then its previous log"
    );
    let service = walked[0].json();
    assert_eq!(service["ready"], "0/1");
    assert_eq!(
        service["why_none"],
        "no pod matching app=worker is ready: worker-5c4d3e9f1-q8zt1 (CrashLoopBackOff)"
    );
    assert_eq!(service["ports"][0]["target_port"], 8080);
    let log = walked[2].json();
    assert!(
        log["text"]
            .as_str()
            .unwrap()
            .contains("password authentication failed for user \"worker\""),
        "{log}"
    );
}

#[test]
fn the_api_ingress_serves_its_ready_pods_over_the_certificate_cert_manager_keeps() {
    let ingress = ok(&[
        "k8s",
        "ingress",
        "get",
        "prod/web/api",
        "--fields",
        "rules,tls",
    ]);
    assert_eq!(
        ingress,
        json!({"rules": [{"host": "api.contoso.example", "path": "/", "service": "prod/web/api", "port": "http", "ready": "3/3"}],
            "tls": [{"hosts": ["api.contoso.example"], "secret": "prod/web/api-tls", "exists": true}]})
    );
}

#[test]
fn the_web_pods_egress_is_denied_but_for_dns_the_namespace_and_private_addresses() {
    let policies = ok(&[
        "k8s",
        "networkpolicy",
        "list",
        "--pod",
        "prod/web/worker-5c4d3e9f1-q8zt1",
        "--fields",
        "id,default_deny",
    ]);
    assert_eq!(
        policies,
        json!([{"id": "prod/web/default-deny-egress", "default_deny": true},
            {"id": "prod/web/allow-egress", "default_deny": false}])
    );
    let allowed = ok(&["k8s", "networkpolicy", "get", "prod/web/allow-egress"]);
    assert_eq!(
        allowed["rules"][2],
        json!({"direction": "egress", "peers": ["cidr 10.0.0.0/8"], "ports": ["5432/TCP", "443/TCP"]})
    );

    let cluster = ok(&["k8s", "context", "get"]);
    assert_eq!(
        cluster,
        json!({"name": "prod", "context": "aks-contoso-prod", "version": "v1.30.4",
            "dns_ip": "10.0.0.10", "cni": "azure-cni", "nodes": 1})
    );
}
