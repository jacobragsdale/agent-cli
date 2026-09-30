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
