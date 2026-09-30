//! The fixtures build against `fixtures/world`, as an agent trial runs it: the
//! real binary, the recorded answers, the fake `az` and `kubectl` on PATH, the
//! clock frozen where the world was recorded (what `scripts/trial-env.sh`
//! prints). Compiled only with the `fixtures` feature.
#![cfg(feature = "fixtures")]

use std::process::Command;

use agent_cli_core::Domain;
use agent_cli_core::testing::{non_utc_times, printed_command_problems};
use serde_json::{Value, json};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
/// `AGENT_CLI_NOW` in `scripts/trial-env.sh`.
const NOW: &str = "2026-09-29T12:00:00Z";
/// The binary's registry, to check every command line it prints.
const DOMAINS: &[Domain] = &[
    agent_cli_ado::DOMAIN,
    agent_cli_azure::KV,
    agent_cli_azure::ACR,
    agent_cli_azure::AKS,
    agent_cli_k8s::K8S,
    agent_cli_sql::DOMAIN,
    agent_cli_airflow::DOMAIN,
    agent_cli_dd::DOMAIN,
];

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Ran {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|_| panic!("not JSON: {}", self.stdout))
    }
}

fn agent_cli(args: &[&str]) -> Ran {
    let world = format!("{REPO}/fixtures/world");
    let path = format!(
        "{REPO}/scripts/fake:{}",
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_agent-cli"))
        .args(args)
        .env("AGENT_CLI_FIXTURES", &world)
        .env("AGENT_CLI_CONFIG", format!("{world}/config.toml"))
        .env("AGENT_CLI_NOW", NOW)
        .env("PATH", path)
        .env("AZURE_CONFIG_DIR", format!("{world}/.azure-unused"))
        .env("AIRFLOW_PROD_PASSWORD", "stand-in")
        .env("DD_ACCESS_TOKEN", "fixture-dd-token")
        .env_remove("AZURE_DEVOPS_EXT_PAT")
        .env_remove("AGENT_CLI_READ_ONLY")
        .output()
        .unwrap();
    let ran = Ran {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    let problems = printed_command_problems(DOMAINS, &ran.stderr);
    assert!(problems.is_empty(), "{args:?}: {problems:?}");
    if let Ok(printed) = serde_json::from_str::<Value>(&ran.stdout) {
        assert!(
            non_utc_times(&printed).is_empty(),
            "{args:?}: {}",
            ran.stdout
        );
    }
    ran
}

fn ok(args: &[&str]) -> Value {
    let ran = agent_cli(args);
    assert_eq!(ran.code, 0, "{args:?}: {}{}", ran.stdout, ran.stderr);
    ran.json()
}

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
}

#[test]
fn the_rest_of_the_world_answers_what_a_trial_is_likely_to_ask() {
    for args in [
        &["ado", "run", "get", "8809", "--fields", "failed"][..],
        &["ado", "run", "logs", "8809"],
        &["ado", "run", "list", "--since", "1d"],
        &["ado", "run", "list", "--branch", "v1.4.2"],
        &["ado", "pr", "get", "431"],
        &["ado", "pr", "list"],
        &["ado", "workitem", "get", "AB#1207"],
        &[
            "ado",
            "workitem",
            "list",
            "--assignee",
            "@me",
            "--iteration",
            "@current",
        ],
        &["ado", "approval", "list"],
        &["ado", "pipeline", "list"],
        &["acr", "repo", "list"],
        &["kv", "secret", "list", "--expires-within", "30d"],
        &["kv", "version", "list", "kv-contoso-prod/db-password"],
        &["aks", "cluster", "list"],
        &["k8s", "pod", "list"],
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
        &["dd", "monitor", "list"],
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
    let expiring = ok(&[
        "kv",
        "secret",
        "list",
        "--expires-within",
        "30d",
        "--fields",
        "id",
    ]);
    assert_eq!(
        expiring,
        json!([{"id": "kv-contoso-prod/db-password"}, {"id": "kv-contoso-prod/worker-db-password"}]),
        "at the world's frozen now"
    );
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
    let refused = agent_cli(&["airflow", "run", "retry", &run]);
    assert_eq!(refused.code, 2, "prod is read_only: {}", refused.stderr);
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
