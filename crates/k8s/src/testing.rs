//! What the crate's tests share: the fake kubectl's scopes, and runs of
//! the domain over them.

use agent_cli_core::Setup;
use agent_cli_core::testing::{FakeTransport, Outcome};

use crate::K8S;

/// The fake kubectl's two contexts, one with three namespaces.
pub const SCOPES: &str = r#"
[[k8s.scope]]
name = "qa"
context = "aks-qa"
namespaces = ["dev", "qa", "uat"]

[[k8s.scope]]
name = "prod"
context = "aks-prod"
namespaces = "prod"

[[k8s.scope]]
name = "all"
context = "aks-qa"
"#;

pub fn k8s(argv: &[&str]) -> Outcome {
    k8s_with(argv, SCOPES)
}

pub fn k8s_with(argv: &[&str], config: &str) -> Outcome {
    agent_cli_core::testing::run(
        &[K8S],
        argv,
        Setup::fake(FakeTransport::default()).with_config(config),
    )
}

const DEV: [&str; 4] = ["--cluster", "qa", "--namespace", "dev"];

fn with(argv: &[&str]) -> Vec<String> {
    argv.iter()
        .chain(DEV.iter())
        .map(|word| (*word).to_owned())
        .collect()
}

pub fn run(argv: &[&str]) -> agent_cli_core::testing::Outcome {
    let argv = with(argv);
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    k8s(&argv)
}

/// `testing::assert_dry_run` over [`SCOPES`], which it cannot be given:
/// `--dry-run` goes before any `--`, kubectl's reads run against the fake,
/// and the first change is planned, never run.
pub fn dry_run(argv: &[&str]) -> Vec<serde_json::Value> {
    let mut argv = argv.to_vec();
    let at = argv
        .iter()
        .position(|arg| *arg == "--")
        .unwrap_or(argv.len());
    argv.insert(at, "--dry-run");
    let outcome = k8s(&argv);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let printed = outcome.json();
    assert_eq!(
        printed["dry_run"], true,
        "never reached ctx.write: {outcome:?}"
    );
    printed["would"].as_array().cloned().unwrap_or_default()
}
