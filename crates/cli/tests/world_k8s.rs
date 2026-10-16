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
        &["k8s", "pvc", "list"],
        &["k8s", "pv", "list"],
        &[
            "k8s",
            "pv",
            "get",
            "prod/pvc-7c3e9a1b-4d2f-4e8a-9b6c-1f0e2d3c4b5a",
        ],
        &["k8s", "storageclass", "list"],
        &["k8s", "event", "list", "--object", "pvc/data-orders-db-0"],
        &["k8s", "statefulset", "list"],
        &["k8s", "job", "list"],
        &["k8s", "cronjob", "list"],
        &["k8s", "daemonset", "get", "prod/kube-system/kube-proxy"],
        &["k8s", "node", "list"],
        &["k8s", "node", "get", "prod/aks-system-31415926-vmss000001"],
        &["k8s", "namespace", "list"],
        &["k8s", "quota", "list"],
        &["k8s", "serviceaccount", "list"],
        &["k8s", "access", "list"],
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
    assert_eq!(nodes.as_array().unwrap().len(), 6, "{nodes}");
    assert_eq!(
        nodes[4],
        json!({"id": "prod/aks-user-27182818-vmss000001", "ready": false})
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
            "dns_ip": "10.0.0.10", "cni": "azure-cni", "nodes": 6})
    );
}

#[test]
fn the_database_pod_stuck_creating_has_its_disk_still_on_the_old_node_which_is_not_ready() {
    let walked = follow(&["k8s", "pvc", "get", "prod/web/data-orders-db-0"]);
    assert_eq!(
        walked.len(),
        4,
        "pvc get, the node holding the disk, its pool, the cluster's events"
    );
    let claim = walked[0].json();
    assert_eq!(claim["used_by"], json!(["prod/web/orders-db-0"]));
    assert_eq!(claim["attached_to"], "prod/aks-user-27182818-vmss000001");
    assert_eq!(
        claim["pv"]["disk"],
        "/subscriptions/00000000-0000-4000-8000-000000005b01/resourceGroups/MC_rg-contoso-prod_aks-contoso-prod_westeurope/providers/Microsoft.Compute/disks/pvc-7c3e9a1b-4d2f-4e8a-9b6c-1f0e2d3c4b5a"
    );
    assert_eq!(claim["class"]["parameters"]["skuname"], "Premium_LRS");
    assert!(
        claim["events"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("Multi-Attach error for volume \"pvc-7c3e9a1b"),
        "{claim}"
    );
    let node = walked[1].json();
    assert_eq!(node["id"], "prod/aks-user-27182818-vmss000001");
    assert_eq!(
        (&node["ready"], &node["schedulable"]),
        (&json!(false), &json!(false))
    );
    assert_eq!(
        node["conditions"][2],
        json!({"type": "Ready", "status": "Unknown", "reason": "NodeStatusUnknown",
            "message": "Kubelet stopped posting node status."})
    );
    assert_eq!(
        node["vm"],
        "/subscriptions/00000000-0000-4000-8000-000000005b01/resourceGroups/MC_rg-contoso-prod_aks-contoso-prod_westeurope/providers/Microsoft.Compute/virtualMachineScaleSets/aks-user-27182818-vmss/virtualMachines/1"
    );
    assert_eq!(node["nodepool"], "aks-contoso-prod/user");
    assert_eq!(walked[2].json()["id"], "aks-contoso-prod/user");

    let pod = ok(&[
        "k8s",
        "pod",
        "get",
        "prod/web/orders-db-0",
        "--fields",
        "status,node,owner",
    ]);
    assert_eq!(
        pod,
        json!({"status": "ContainerCreating", "node": "aks-user-27182818-vmss000002",
            "owner": "StatefulSet/orders-db"})
    );
    let events = ok(&[
        "k8s",
        "event",
        "list",
        "--object",
        "node/prod/aks-user-27182818-vmss000001",
        "--fields",
        "reason",
    ]);
    assert_eq!(
        events,
        json!([{"reason": "NodeNotReady"}, {"reason": "NodeNotSchedulable"}])
    );
}

#[test]
fn the_api_deployment_rolled_from_v1_4_1_to_v1_4_2_and_the_worker_leads_to_its_crashing_pod() {
    let api = ok(&[
        "k8s",
        "deployment",
        "get",
        "prod/web/api",
        "--fields",
        "revisions,containers",
    ]);
    assert_eq!(
        api["revisions"],
        json!([
            {"revision": 7, "images": ["contosoacr.azurecr.io/api:v1.4.2"],
                "change_cause": "image api:v1.4.2 (api-ci run 8812)", "created": "2026-09-28T21:34:12Z", "replicas": 3},
            {"revision": 6, "images": ["contosoacr.azurecr.io/api:v1.4.1"],
                "created": "2026-09-15T16:20:00Z", "replicas": 0},
        ])
    );
    assert_eq!(
        api["containers"][0]["env"][0],
        json!({"name": "DB_USER", "secret": "prod/web/api-env", "key": "DB_USER"})
    );

    let walked = follow(&["k8s", "deployment", "get", "prod/web/worker"]);
    assert_eq!(
        walked.len(),
        3,
        "deployment get, its pod, then its previous log"
    );
    assert_eq!(walked[0].json()["strategy"], "Recreate");
    assert!(
        walked[2].json()["text"]
            .as_str()
            .unwrap()
            .contains("password authentication failed for user \"worker\"")
    );
}

#[test]
fn the_database_statefulset_is_unready_and_its_budget_allows_no_eviction() {
    let walked = follow(&["k8s", "statefulset", "get", "prod/web/orders-db"]);
    assert_eq!(walked.len(), 2, "statefulset get, then its pod");
    let set = walked[0].json();
    assert_eq!(set["ready"], "0/1");
    assert_eq!(
        set["claims"][0]["pvcs"],
        json!(["prod/web/data-orders-db-0"])
    );
    assert_eq!(walked[1].json()["status"], "ContainerCreating");
    let budgets = ok(&[
        "k8s",
        "pdb",
        "list",
        "--fields",
        "id,selector,allowed_disruptions",
    ]);
    assert_eq!(
        budgets,
        json!([{"id": "prod/web/orders-db", "selector": "app=orders-db", "allowed_disruptions": 0}])
    );
}

#[test]
fn the_failed_export_leads_by_notes_from_its_cronjob_to_the_timeout_in_its_log() {
    let walked = follow(&["k8s", "cronjob", "get", "prod/web/orders-export"]);
    assert_eq!(
        walked.len(),
        3,
        "cronjob get, the failed job, then its pod's log"
    );
    let cronjob = walked[0].json();
    assert_eq!(cronjob["last_success"], "2026-09-28T01:06:12Z");
    assert_eq!(cronjob["jobs"][0]["status"], "Failed");
    let job = walked[1].json();
    assert_eq!(job["conditions"][0]["reason"], "BackoffLimitExceeded");
    assert_eq!(job["pods"][0]["owner"], "CronJob/orders-export");
    assert!(
        walked[2].json()["text"]
            .as_str()
            .unwrap()
            .contains("in /app/src/Orders/OrderClient.cs:line 19")
    );
}

#[test]
fn reports_api_has_pending_pods_no_node_fits_and_its_autoscaler_is_at_its_maximum() {
    let walked = follow(&["k8s", "deployment", "get", "prod/web/reports-api"]);
    assert_eq!(walked.len(), 2, "deployment get, then a pending pod");
    assert_eq!(walked[0].json()["ready"], "3/5");
    let pod = walked[1].json();
    assert_eq!(pod["status"], "Pending");
    assert_eq!(pod["events"][0]["reason"], "FailedScheduling");
    assert!(
        pod["events"][0]["message"].as_str().unwrap().starts_with(
            "0/6 nodes are available: 1 node(s) were unschedulable, 2 Insufficient cpu"
        ),
        "{pod}"
    );
    let autoscalers = ok(&["k8s", "hpa", "list", "--fields", "id,max,current,metrics"]);
    assert_eq!(
        autoscalers,
        json!([{"id": "prod/web/reports-api", "max": 5, "current": 5,
            "metrics": [{"metric": "cpu", "current": "96%", "target": "70%"}]}])
    );
    let daemonsets = ok(&[
        "k8s",
        "daemonset",
        "list",
        "--namespace",
        "kube-system",
        "--fields",
        "id,unavailable",
    ]);
    assert_eq!(
        daemonsets[0],
        json!({"id": "prod/kube-system/azure-cns", "unavailable": 1})
    );
    // Why the pending pods fit nowhere: the system pool's taint, the
    // cordoned node, and the other two user nodes with 160m of CPU left.
    let nodes = ok(&[
        "k8s",
        "node",
        "list",
        "--fields",
        "name,ready,schedulable,taints,cpu",
    ]);
    let user: Vec<_> = nodes.as_array().unwrap()[3..].to_vec();
    assert_eq!(
        user[0],
        json!({"name": "aks-user-27182818-vmss000000", "ready": true, "schedulable": true,
            "cpu": {"requested": "3700m", "allocatable": "3860m", "requested_percent": 96,
                "usage": "3412m", "usage_percent": 88}})
    );
    assert_eq!(user[1]["schedulable"], false);
    assert_eq!(user[2]["cpu"]["requested_percent"], 96);
    assert_eq!(
        nodes[0]["taints"],
        json!(["CriticalAddonsOnly=true:NoSchedule"])
    );
    let pods = ok(&[
        "k8s",
        "pod",
        "list",
        "reports-api",
        "--fields",
        "name,cpu_usage",
    ]);
    assert_eq!(
        pods[0],
        json!({"name": "reports-api-6f7d8c9b4-h2k8m", "cpu_usage": "958m"})
    );
}

#[test]
fn the_web_namespace_has_a_quota_and_its_service_accounts_say_what_they_may_do() {
    let namespaces = ok(&[
        "k8s",
        "namespace",
        "list",
        "--fields",
        "name,quota,in_scope",
    ]);
    assert_eq!(
        namespaces[4],
        json!({"name": "web", "in_scope": true,
            "quota": {"pods": "10/30", "requests.cpu": "6500m/12", "requests.memory": "7Gi/24Gi"}})
    );
    assert_eq!(namespaces[2], json!({"name": "ingest", "in_scope": false}));
    let limits = ok(&[
        "k8s",
        "quota",
        "list",
        "--fields",
        "from,resource,default_request",
    ]);
    assert_eq!(
        limits[3],
        json!({"from": "LimitRange/web-defaults", "resource": "cpu", "default_request": "100m"})
    );
    let pod = ok(&[
        "k8s",
        "pod",
        "get",
        "prod/web/worker-5c4d3e9f1-q8zt1",
        "--fields",
        "service_account",
    ]);
    let account = ok(&[
        "k8s",
        "serviceaccount",
        "get",
        pod["service_account"].as_str().unwrap(),
    ]);
    assert!(account.get("client_id").is_none(), "{account}");
    assert_eq!(account["pods"], json!(["prod/web/worker-5c4d3e9f1-q8zt1"]));
    assert_eq!(
        account["can"],
        json!([{"resource": "configmaps", "names": ["api-config"], "verbs": ["get"]},
            {"resource": "secrets", "names": ["worker-db"], "verbs": ["get"]}])
    );
    let accounts = ok(&["k8s", "serviceaccount", "list", "--fields", "id,client_id"]);
    assert_eq!(
        accounts[2],
        json!({"id": "prod/web/notifier", "client_id": "00000000-0000-4000-8000-00000000b0f1"})
    );
    // The client id is what arm identity get takes, and the serviceaccount
    // its federated credential names is what serviceaccount get takes.
    let identity = ok(&[
        "arm",
        "identity",
        "get",
        accounts[2]["client_id"].as_str().unwrap(),
    ]);
    let subject = identity["federated_credentials"][0]["serviceaccount"]
        .as_str()
        .unwrap();
    let back = ok(&["k8s", "serviceaccount", "get", subject, "--fields", "id"]);
    assert_eq!(back, json!({"id": "prod/web/notifier"}));
    let mine = ok(&[
        "k8s", "access", "list", "--as", "@me", "--fields", "resource",
    ]);
    assert_eq!(mine[0], json!({"resource": "*.*"}));
}

#[test]
fn a_web_pod_answers_env_and_resolv_conf_and_every_write_answers_as_kubectl_would() {
    let pod = "prod/web/api-7d9f8c6b5-x2k4q";
    let resolv = ok(&[
        "k8s",
        "pod",
        "run",
        pod,
        "--yes",
        "--",
        "cat",
        "/etc/resolv.conf",
    ]);
    assert_eq!(resolv["container"], "api");
    assert!(
        resolv["stdout"]
            .as_str()
            .unwrap()
            .contains("nameserver 10.0.0.10"),
        "kube-dns, as context get has it: {resolv}"
    );
    let env = ok(&["k8s", "pod", "run", pod, "--yes", "--", "env"]);
    assert!(
        env["stdout"].as_str().unwrap().contains("DB_USER=api"),
        "{env}"
    );

    let rolled = ok(&[
        "k8s",
        "deployment",
        "update",
        "prod/web/api",
        "--revision",
        "6",
        "--yes",
    ]);
    assert_eq!(
        rolled["images"],
        json!(["contosoacr.azurecr.io/api:v1.4.1"])
    );
    assert_eq!(rolled["previous"], 7);
    let missing = agent_cli(&[
        "k8s",
        "deployment",
        "update",
        "prod/web/api",
        "--revision",
        "5",
        "--yes",
    ]);
    assert_eq!(missing.code, 2, "{}", missing.stderr);
    assert!(
        missing.stderr.contains("it keeps 7, 6"),
        "{}",
        missing.stderr
    );

    let run = agent_cli(&["k8s", "cronjob", "run", "prod/web/orders-export"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let id = run.json()["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("prod/web/orders-export-manual-"), "{id}");
    assert!(
        run.stderr
            .contains(&format!("[next: agent-cli k8s job get {id}]")),
        "{}",
        run.stderr
    );

    for args in [
        &[
            "k8s",
            "job",
            "delete",
            "prod/web/orders-export-29844060",
            "--yes",
        ][..],
        &[
            "k8s",
            "statefulset",
            "restart",
            "prod/web/orders-db",
            "--yes",
        ],
        &[
            "k8s",
            "statefulset",
            "scale",
            "prod/web/orders-db",
            "--replicas",
            "1",
            "--yes",
        ],
        &[
            "k8s",
            "daemonset",
            "restart",
            "prod/kube-system/azure-cns",
            "--yes",
        ],
        &[
            "k8s",
            "node",
            "update",
            "prod/aks-user-27182818-vmss000001",
            "--schedulable",
            "true",
            "--yes",
        ],
    ] {
        ok(args);
    }
}
