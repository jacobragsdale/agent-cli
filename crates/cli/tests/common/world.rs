//! The fixtures build against `fixtures/world`, as an agent trial runs it: the
//! real binary, the recorded answers, the fake `az` and `kubectl` on PATH, the
//! clock frozen where the world was recorded (what `scripts/trial-env.sh`
//! prints). Each `tests/world_*.rs` includes this file with
//! `#[path = "common/world.rs"] mod world;`.

use std::process::Command;

use agent_cli_core::Domain;
use agent_cli_core::testing::{next_command, non_utc_times, printed_command_problems};
use serde_json::Value;

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

pub struct Ran {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|_| panic!("not JSON: {}", self.stdout))
    }
}

pub fn agent_cli(args: &[&str]) -> Ran {
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
        .env_remove("AGENT_CLI_FIXTURES_MATCH")
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

/// Runs `args`, then each command its `[next: …]` note names, until one
/// names none: the trace an agent walks by notes alone, one run per step.
#[allow(dead_code)] // not every world_*.rs walks notes
pub fn follow(args: &[&str]) -> Vec<Ran> {
    let mut argv: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let mut walked = Vec::new();
    loop {
        let ran = agent_cli(&argv.iter().map(String::as_str).collect::<Vec<_>>());
        let next = next_command(&ran.stderr);
        walked.push(ran);
        let Some(next) = next else { return walked };
        assert!(walked.len() < 10, "notes loop: {argv:?} → {next:?}");
        argv = next;
    }
}

pub fn ok(args: &[&str]) -> Value {
    let ran = agent_cli(args);
    assert_eq!(ran.code, 0, "{args:?}: {}{}", ran.stdout, ran.stderr);
    ran.json()
}
