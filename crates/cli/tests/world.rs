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
