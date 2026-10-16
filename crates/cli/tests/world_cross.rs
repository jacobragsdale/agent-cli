//! What the contoso world answers across domains: the traces that hop from
//! one domain to the next, and what a miss says. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::{Value, json};
use world::{agent_cli, follow, ok};

#[test]
fn the_deploy_trace_runs_from_the_cluster_to_the_work_items() {
    let deployments = ok(&["k8s", "deployment", "list", "--fields", "id,images"]);
    let api = &deployments[0];
    assert_eq!(api["id"], "prod/web/api");
    assert_eq!(
        api["images"][0]["image"],
        "contosoacr.azurecr.io/api:v1.4.2"
    );

    let tags = ok(&["acr", "tag", "list", "api", "--fields", "id,digest"]);
    assert_eq!(tags[0]["id"], "contosoacr.azurecr.io/api:v1.4.2");
    assert_eq!(
        tags[0]["digest"], api["images"][0]["digest"],
        "prod runs the digest the registry holds for the tag"
    );
    let manifest = ok(&[
        "acr",
        "manifest",
        "get",
        "contosoacr.azurecr.io/api:v1.4.2",
        "--fields",
        "digest",
    ]);
    assert_eq!(manifest["digest"], tags[0]["digest"]);

    let runs = ok(&[
        "ado",
        "run",
        "list",
        "--branch",
        "refs/tags/v1.4.2",
        "--fields",
        "id,result",
    ]);
    assert_eq!(
        runs,
        json!([{"id": 8812, "result": "succeeded"}, {"id": 8809, "result": "failed"}])
    );
    let run = ok(&[
        "ado",
        "run",
        "get",
        "8812",
        "--fields",
        "commit,pr,workitems",
    ]);
    assert_eq!(run["pr"]["id"], 431);
    let workitems: Vec<&Value> = run["workitems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| &w["id"])
        .collect();
    assert_eq!(workitems, [&json!(1207), &json!(1210)]);

    let mine = ok(&[
        "ado",
        "workitem",
        "list",
        "--assignee",
        "@me",
        "--fields",
        "id",
    ]);
    assert_eq!(mine, json!([{"id": 1218}, {"id": 1215}, {"id": 1207}]));
    let unresolved = ok(&[
        "ado",
        "workitem",
        "list",
        "--assignee",
        "@me",
        "--iteration",
        "@current",
        "--state",
        "New",
        "--state",
        "Active",
        "--fields",
        "id",
    ]);
    assert_eq!(unresolved, json!([{"id": 1218}, {"id": 1215}]));
}

#[test]
fn last_nights_failed_dag_run_leads_to_its_exception_and_its_pod() {
    let dags = ok(&["airflow", "dag", "list", "--fields", "id,paused"]);
    assert_eq!(
        dags,
        json!([{"id": "etl_nightly", "paused": false}, {"id": "orders_export", "paused": false},
            {"id": "reports_weekly", "paused": true}])
    );
    let runs = ok(&[
        "airflow",
        "run",
        "list",
        "--dag",
        "etl_nightly",
        "--state",
        "failed",
        "--since",
        "1d",
        "--fields",
        "id",
    ]);
    let run = runs[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(run, "etl_nightly/scheduled__2026-09-29T00:00:00+00:00");
    let failed = ok(&["airflow", "run", "get", &run, "--fields", "failed"]);
    let task = failed["failed"][0].as_str().unwrap().to_owned();
    assert_eq!(task, format!("{run}/load_orders/2"));

    let log = ok(&[
        "airflow",
        "task",
        "logs",
        &task,
        "--tail",
        "20",
        "--fields",
        "error,text",
    ]);
    assert_eq!(
        log["error"],
        "ValueError: order 88123 has no customer_id (at /opt/airflow/dags/etl_nightly.py:42 in load_orders)"
    );
    assert!(
        log["text"]
            .as_str()
            .unwrap()
            .contains("\nValueError: order 88123 has no customer_id\n"),
        "{log}"
    );

    let pod = ok(&["airflow", "task", "get", &task, "--fields", "pod"]);
    let pod = pod["pod"].as_str().unwrap();
    assert_eq!(pod, "prod/web/etl-nightly-load-orders-q8x1k2vz");
    let pod_log = ok(&[
        "k8s", "pod", "logs", pod, "--tail", "20", "--fields", "pod,text",
    ]);
    assert_eq!(pod_log["pod"], pod, "the printed ref is the id k8s takes");
    assert!(
        pod_log["text"]
            .as_str()
            .unwrap()
            .contains("ValueError: order 88123 has no customer_id")
    );
    for args in [
        &["k8s", "event", "list", "--pod", pod][..],
        &["airflow", "dag", "get", "etl_nightly"],
        &["airflow", "task", "list", "etl_nightly/latest"],
        &[
            "airflow", "run", "list", "--state", "failed", "--since", "1d",
        ],
        &["airflow", "import-error", "get", "12"],
        &["doctor", "airflow"],
    ] {
        ok(args);
    }
    let broken = ok(&["airflow", "import-error", "list", "--fields", "file,error"]);
    assert_eq!(
        broken,
        json!([{"file": "customer_sync.py", "error": "ModuleNotFoundError: No module named 'contoso_crm'"}])
    );
}

#[test]
fn datadog_shows_the_alert_at_the_deploy_the_errors_and_the_pod_to_hop_to() {
    let rolled_out =
        ok(&["k8s", "deployment", "list", "--fields", "id,updated"])[0]["updated"].clone();
    assert_eq!(rolled_out, "2026-09-28T21:34:40Z");
    let monitor = ok(&[
        "dd",
        "monitor",
        "get",
        "4711",
        "--all-groups",
        "--fields",
        "name,state,triggered,groups",
    ]);
    assert_eq!(monitor["name"], "[prod] api error rate above 5%");
    assert_eq!(
        monitor["triggered"], "2026-09-28T21:36:00Z",
        "two minutes after api v1.4.2 rolled out"
    );
    assert_eq!(monitor["state"], "OK");
    assert_eq!(monitor["groups"][0]["resolved"], "2026-09-28T21:58:00Z");
    assert_eq!(monitor["groups"][0]["pod"], "prod/web/api-7d9f8c6b5-x2k4q");

    let spike = ok(&[
        "dd",
        "metric",
        "get",
        "sum:trace.http.request.errors{service:api,env:prod}.as_count()",
        "--since",
        "1d",
        "--fields",
        "max,max_at,last",
    ]);
    assert_eq!(
        spike,
        json!([{"max": 64.0, "max_at": "2026-09-28T21:40:00Z", "last": 0.0}]),
        "api's request errors spiked at the rollout and are gone now"
    );

    let alerting = ok(&[
        "dd", "monitor", "list", "--state", "Alert", "--fields", "id,name",
    ]);
    assert_eq!(
        alerting,
        json!([{"id": 4712, "name": "[prod] worker crash-looping"}])
    );

    let errors = ok(&[
        "dd",
        "log",
        "list",
        "--status",
        "error",
        "--since",
        "2026-09-28T21:30:00Z",
        "--until",
        "2026-09-28T22:30:00Z",
        "--fields",
        "time,service,pod,message",
    ]);
    let first_crash = errors
        .as_array()
        .unwrap()
        .iter()
        .rfind(|row| row["service"] == "worker")
        .unwrap();
    assert_eq!(first_crash["time"], "2026-09-28T21:36:05.874Z");
    assert_eq!(first_crash["pod"], "prod/web/worker-5c4d3e9f1-q8zt1");
    assert!(
        first_crash["message"]
            .as_str()
            .unwrap()
            .contains("password authentication failed")
    );

    let pod = first_crash["pod"].as_str().unwrap();
    let hop = ok(&["k8s", "pod", "get", pod, "--fields", "id,status,restarts"]);
    assert_eq!(hop["id"], pod, "a dd row's pod is the k8s pod id");
    assert_eq!(hop["restarts"], 23);
}

#[test]
fn a_request_the_world_did_not_record_says_which_one_it_wanted() {
    let ran = agent_cli(&["ado", "run", "get", "9999"]);
    assert_eq!(ran.code, 1, "{}", ran.stderr);
    assert!(
        ran.stderr.starts_with(
            "error: fixtures: no recorded answer for GET https://dev.azure.com/contoso/Fabrikam/_apis/build/builds/9999?api-version=7.1; closest recorded: "
        ),
        "{}",
        ran.stderr
    );
}

#[test]
fn a_failed_task_leads_to_its_line_in_the_dag_and_the_same_line_in_the_repo() {
    let log = ok(&[
        "airflow",
        "task",
        "logs",
        "etl_nightly/latest/load_orders/2",
        "--fields",
        "at",
    ]);
    let at = log["at"].as_str().unwrap().to_owned();
    assert_eq!(at, "etl_nightly:42");
    let source = ok(&[
        "airflow",
        "source",
        "get",
        &at,
        "--fields",
        "repo_file,text",
    ]);
    let repo_file = source["repo_file"].as_str().unwrap().to_owned();
    assert_eq!(repo_file, "airflow-dags:dags/etl_nightly.py:42");
    let file = ok(&["ado", "file", "get", &repo_file, "--fields", "text"]);
    // Both number their lines; the code after the number is what matters.
    let line_42 = |text: &Value| {
        text.as_str()
            .unwrap()
            .lines()
            .find_map(|line| line.trim_start().strip_prefix("42 "))
            .map(|code| code.trim().to_owned())
            .unwrap()
    };
    assert_eq!(
        line_42(&source["text"]),
        line_42(&file["text"]),
        "the repo holds the code Airflow ran"
    );
    assert_eq!(
        line_42(&file["text"]),
        r#"raise ValueError(f"order {order_id} has no customer_id")"#,
        "the line the pod's traceback prints"
    );
}

/// F10: an api exception in Datadog, to its line at the deployed commit, the
/// change that last touched that file, and the work behind the change.
#[test]
fn a_production_exception_leads_to_its_line_its_pull_request_and_work_items() {
    let spans = ok(&[
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
        "--fields",
        "at",
    ]);
    let at = spans[0]["at"].as_str().unwrap();
    let file = ok(&["ado", "file", "get", at, "--fields", "id,text"]);
    assert_eq!(
        file["id"],
        "api@4be1c0d2e8f1a9b3c5d7e9f1a2b3c4d5e6f7a8b9:src/Orders/OrderClient.cs"
    );
    assert!(
        file["text"]
            .as_str()
            .unwrap()
            .lines()
            .any(|line| line.starts_with("19 ") && line.contains("await http.GetAsync")),
        "{}",
        file["text"]
    );

    let commits = ok(&[
        "ado",
        "commit",
        "list",
        "api:src/Orders/OrderClient.cs",
        "--fields",
        "commit,pr",
    ]);
    assert_eq!(commits[0]["pr"]["id"], 431);
    let pr = ok(&["ado", "pr", "get", "431", "--fields", "work_items"]);
    assert_eq!(pr["work_items"], json!([1207, 1210]));
}

/// F11: the crash-loop alert, by its note, to the refused password; Key Vault
/// names the expired secret, and the reverse lookup the deployment to restart
/// once it is rotated, whose rollout has given up until then.
#[test]
fn an_expired_secret_leads_from_the_alert_to_the_deployment_that_reads_it() {
    let walked = follow(&["dd", "monitor", "get", "4712"]);
    assert_eq!(walked.len(), 2, "monitor get, then its log search");
    assert!(
        walked[1]
            .stdout
            .contains("password authentication failed for user"),
        "{}",
        walked[1].stdout
    );

    let secrets = ok(&[
        "kv",
        "secret",
        "list",
        "--expires-within",
        "30d",
        "--fields",
        "id,expires",
    ]);
    assert!(
        secrets.as_array().unwrap().contains(&json!({
            "id": "kv-contoso-prod/worker-db-password",
            "expires": "2026-09-20T09:00:00Z"
        })),
        "{secrets}"
    );
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
    let restart = agent_cli(&[
        "k8s",
        "deployment",
        "restart",
        "prod/web/worker",
        "--dry-run",
    ]);
    assert_eq!(restart.code, 0, "{}", restart.stderr);
    let wait = agent_cli(&["k8s", "deployment", "wait", "prod/web/worker"]);
    assert_eq!(wait.code, 1, "{}", wait.stderr);
    assert!(
        wait.stderr
            .contains("hint: agent-cli k8s pod logs prod/web/worker-5c4d3e9f1-q8zt1 --previous"),
        "{}",
        wait.stderr
    );
}

/// F12: a DAG missing from the list, by notes alone, to the import in the
/// repository that breaks it.
#[test]
fn an_import_error_leads_by_notes_alone_to_its_line_in_the_repository() {
    let walked = follow(&["airflow", "import-error", "get", "12"]);
    assert_eq!(walked.len(), 2, "import-error get, then ado file get");
    let file = walked[1].json();
    assert_eq!(file["id"], "airflow-dags:dags/customer_sync.py");
    assert!(
        file["text"]
            .as_str()
            .unwrap()
            .contains("5  from contoso_crm import Client"),
        "{}",
        file["text"]
    );
}

/// F13: ship and verify, by notes: the run to its wait, the rollout to the
/// service's health since it rolled out.
#[test]
fn a_new_build_is_waited_for_and_its_rollout_verified_by_notes() {
    let walked = follow(&["ado", "run", "create", "--pipeline", "api-ci"]);
    assert_eq!(walked.len(), 2, "run create, then run wait");
    assert_eq!(walked[1].code, 0, "{}", walked[1].stderr);
    assert_eq!(walked[1].json()["result"], "succeeded");

    let walked = follow(&["k8s", "deployment", "wait", "prod/web/api"]);
    assert_eq!(walked.len(), 2, "deployment wait, then service get");
    assert_eq!(walked[0].json()["rolled_out"], "2026-09-28T21:34:40Z");
    let service = walked[1].json();
    assert_eq!(service["since"], "2026-09-28T21:34:40Z");
    assert_eq!(service["errors"], 212);
}

#[test]
fn an_order_missing_from_search_leads_to_the_indexer_the_runbook_and_last_nights_load() {
    let missing = agent_cli(&[
        "aisearch",
        "document",
        "get",
        "srch-contoso-prod/orders/88123",
    ]);
    assert_eq!(missing.code, 4, "{}{}", missing.stdout, missing.stderr);
    assert!(
        missing
            .stderr
            .contains("agent-cli aisearch indexer list --service srch-contoso-prod --failing"),
        "{}",
        missing.stderr
    );
    let failing = ok(&[
        "aisearch",
        "indexer",
        "list",
        "--service",
        "srch-contoso-prod",
        "--failing",
        "--fields",
        "id,failed",
    ]);
    assert_eq!(
        failing,
        json!([{"id": "srch-contoso-prod/orders-sql", "failed": 1}])
    );
    let indexer = ok(&[
        "aisearch",
        "indexer",
        "get",
        "srch-contoso-prod/orders-sql",
        "--fields",
        "errors",
    ]);
    assert_eq!(indexer["errors"][0]["key"], "88123");
    assert!(
        indexer["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("customer_id"),
        "{indexer}"
    );

    let runbooks = ok(&[
        "confluence",
        "page",
        "list",
        "runbook etl_nightly",
        "--fields",
        "id,title",
    ]);
    assert_eq!(runbooks[0]["id"], "1101", "{runbooks}");
    let section = ok(&[
        "confluence",
        "page",
        "get",
        "1101",
        "--section",
        "An order without customer_id",
        "--fields",
        "body",
    ]);
    let steps = section["body"].as_str().unwrap();
    assert!(
        steps.contains("CRM") && steps.contains("orders-sql"),
        "the runbook says to fix the order, retry the load and re-run the indexer: {steps}"
    );

    let log = ok(&[
        "airflow",
        "task",
        "logs",
        "etl_nightly/scheduled__2026-09-29T00:00:00+00:00/load_orders/2",
        "--tail",
        "20",
        "--fields",
        "error,text",
    ]);
    assert!(
        log["error"]
            .as_str()
            .unwrap()
            .contains("order 88123 has no customer_id"),
        "the search miss and last night's failed load are the same order: {log}"
    );
}

/// The `id` of the one row of `listed` whose `field` ends with `suffix`.
fn id_where(listed: &Value, field: &str, suffix: &str) -> String {
    let rows: Vec<&Value> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row[field]
                .as_str()
                .is_some_and(|held| held.ends_with(suffix))
        })
        .collect();
    assert_eq!(rows.len(), 1, "{field} ending {suffix}: {listed}");
    rows[0]["id"].as_str().unwrap().to_owned()
}

/// Trace 1, private DNS: billing-sync cannot resolve its vault; from the pod
/// the name is a CNAME into the privatelink zone that holds no record for
/// it, because the vault's private endpoint was made without a DNS zone
/// group, though the zone is linked to the AKS vnet.
#[test]
fn a_vault_name_that_does_not_resolve_from_a_pod_leads_to_its_endpoint_without_a_zone_group() {
    let pods = ok(&["k8s", "pod", "list", "billing-sync", "--fields", "id"]);
    let pod = pods[0]["id"].as_str().unwrap();
    let log = ok(&[
        "k8s", "pod", "logs", pod, "--tail", "20", "--fields", "text",
    ]);
    assert!(
        log["text"]
            .as_str()
            .unwrap()
            .contains("Name or service not known (kv-contoso-billing.vault.azure.net:443)"),
        "{log}"
    );

    let lookup = agent_cli(&[
        "k8s",
        "pod",
        "run",
        pod,
        "--yes",
        "--",
        "nslookup",
        "kv-contoso-billing.vault.azure.net",
    ]);
    assert_eq!(lookup.code, 1, "{}", lookup.stderr);
    let stdout = lookup.json()["stdout"].as_str().unwrap().to_owned();
    assert!(
        stdout.contains("canonical name = kv-contoso-billing.privatelink.vaultcore.azure.net.")
            && stdout.contains("NXDOMAIN"),
        "{stdout}"
    );
    let working = ok(&[
        "k8s",
        "pod",
        "run",
        pod,
        "--yes",
        "--",
        "nslookup",
        "kv-contoso-prod.vault.azure.net",
    ]);
    assert!(
        working["stdout"]
            .as_str()
            .unwrap()
            .ends_with("Address: 10.1.4.4"),
        "the other vault resolves privately from the same pod: {working}"
    );

    let endpoints = ok(&["net", "endpoint", "list", "--fields", "id,target"]);
    let endpoint = id_where(&endpoints, "target", "/vaults/kv-contoso-billing");
    let walked = follow(&["net", "endpoint", "get", &endpoint]);
    assert_eq!(walked.len(), 2, "endpoint get, then zone list");
    let got = walked[0].json();
    assert_eq!(got["fqdns"], json!(["kv-contoso-billing.vault.azure.net"]));
    assert!(got.get("dns").is_none(), "no zone group: {got}");
    let zones = walked[1].json();
    assert_eq!(zones[0]["name"], "privatelink.vaultcore.azure.net");
    assert!(
        zones[0]["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["vnet"] == got["vnet"]),
        "the zone is linked where the pods are, so its answer is final: {zones}"
    );
    let records = ok(&[
        "net",
        "record",
        "list",
        zones[0]["id"].as_str().unwrap(),
        "kv-contoso",
        "--fields",
        "name",
    ]);
    assert_eq!(records, json!([{"name": "kv-contoso-prod"}]));
}

/// Trace 2, Multi-Attach: orders-db-0 waits on a disk still attached to the
/// node the pool upgrade cordoned, which has stopped reporting; Azure says
/// the same VM holds the disk.
#[test]
fn a_pod_stuck_creating_leads_to_its_disk_on_a_not_ready_node_and_that_nodes_pool() {
    let pod = ok(&[
        "k8s",
        "pod",
        "get",
        "prod/web/orders-db-0",
        "--fields",
        "status,volumes,events",
    ]);
    assert_eq!(pod["status"], "ContainerCreating");
    assert!(
        pod["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["message"]
                .as_str()
                .unwrap()
                .starts_with("Multi-Attach error")),
        "{pod}"
    );
    let claim = pod["volumes"][0]["source"].as_str().unwrap();

    let walked = follow(&["k8s", "pvc", "get", claim]);
    assert_eq!(
        walked.len(),
        4,
        "pvc get, node get, aks nodepool get, event list"
    );
    let claim = walked[0].json();
    let node = walked[1].json();
    assert_eq!(node["id"], claim["attached_to"]);
    assert_eq!(
        (&node["ready"], &node["schedulable"]),
        (&json!(false), &json!(false)),
        "{node}"
    );
    assert_eq!(walked[2].json()["id"], node["nodepool"]);

    let disk = ok(&[
        "arm",
        "resource",
        "get",
        claim["pv"]["disk"].as_str().unwrap(),
        "--fields",
        "managedBy",
    ]);
    assert_eq!(
        disk["managedBy"], node["vm"],
        "Azure has the disk on the node k8s says holds it"
    );
}

/// Trace 3, pending pods: reports-api's HPA asked for five pods, the two
/// new ones fit on no node, and the user pool's autoscaler is at its
/// maximum.
#[test]
fn pending_pods_lead_to_full_nodes_a_pool_at_its_maximum_and_an_hpa_at_its_own() {
    let walked = follow(&["k8s", "deployment", "get", "prod/web/reports-api"]);
    assert_eq!(walked.len(), 2, "deployment get, then a pending pod");
    let pending = walked[1].json();
    assert_eq!(pending["status"], "Pending");
    assert!(
        pending["events"][0]["message"]
            .as_str()
            .unwrap()
            .contains("2 Insufficient cpu"),
        "{pending}"
    );

    let nodes = ok(&[
        "k8s",
        "node",
        "list",
        "aks-user",
        "--fields",
        "id,schedulable,nodepool,cpu",
    ]);
    let full: Vec<&Value> = nodes
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["schedulable"] == true)
        .collect();
    assert_eq!(full.len(), 2, "{nodes}");
    assert!(
        full.iter()
            .all(|node| node["cpu"]["requested_percent"] == 96),
        "a 1-CPU pod fits on neither: {nodes}"
    );

    let walked = follow(&[
        "aks",
        "nodepool",
        "get",
        full[0]["nodepool"].as_str().unwrap(),
    ]);
    assert_eq!(walked.len(), 2, "nodepool get, then the cluster's events");
    let pool = walked[0].json();
    assert_eq!(pool["count"], pool["autoscale"]["max"], "{pool}");
    assert!(
        walked[1].stdout.contains("max node group size reached"),
        "{}",
        walked[1].stdout
    );

    let autoscalers = ok(&[
        "k8s",
        "hpa",
        "list",
        "--fields",
        "id,max,current,conditions",
    ]);
    let hpa = &autoscalers[0];
    assert_eq!(hpa["id"], "prod/web/reports-api");
    assert_eq!(hpa["current"], hpa["max"]);
    assert!(
        hpa["conditions"].as_array().unwrap().contains(
            &json!({"type": "ScalingLimited", "status": "True", "reason": "TooManyReplicas",
                "message": "the desired replica count is more than the maximum replica count"})
        ),
        "{hpa}"
    );
}

/// Trace 4, workload identity 403: the notifier's Key Vault reads are
/// refused; its serviceaccount names the identity the cluster's issuer is
/// trusted for, which holds no Key Vault role since dana deleted it.
#[test]
fn a_key_vault_403_leads_from_the_pod_to_its_identity_and_who_deleted_its_role() {
    let pods = ok(&["k8s", "pod", "list", "notifier", "--fields", "id"]);
    let pod = pods[0]["id"].as_str().unwrap();
    let log = ok(&[
        "k8s", "pod", "logs", pod, "--tail", "30", "--fields", "text",
    ]);
    let text = log["text"].as_str().unwrap().to_owned();
    assert!(text.contains("ForbiddenByRbac"), "{text}");

    let account = ok(&["k8s", "pod", "get", pod, "--fields", "service_account"]);
    let account = account["service_account"].as_str().unwrap();
    let client = ok(&[
        "k8s",
        "serviceaccount",
        "get",
        account,
        "--fields",
        "client_id",
    ]);
    let identity = ok(&[
        "arm",
        "identity",
        "get",
        client["client_id"].as_str().unwrap(),
    ]);
    assert_eq!(
        identity["federated_credentials"][0]["serviceaccount"], "aks-contoso-prod/web/notifier",
        "the issuer trusts this pod's serviceaccount: {identity}"
    );
    let principal = identity["principal_id"].as_str().unwrap();
    assert!(
        text.contains(&format!("oid={principal}")),
        "the caller Key Vault refused is this identity"
    );

    let roles = ok(&[
        "arm",
        "role",
        "list",
        "--principal",
        principal,
        "--fields",
        "role",
    ]);
    assert_eq!(roles, json!([{"role": "Monitoring Metrics Publisher"}]));
    let log = ok(&[
        "arm",
        "activity",
        "list",
        "--resource",
        "kv-contoso-prod",
        "--fields",
        "when,caller,operation",
    ]);
    assert_eq!(
        log[0],
        json!({"when": "2026-09-28T16:42:11Z", "caller": "dana@contoso.example",
            "operation": "Microsoft.Authorization/roleAssignments/delete"})
    );
}
