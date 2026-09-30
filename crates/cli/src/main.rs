//! `agent-cli`: one CLI that gives coding agents Azure DevOps, Azure,
//! Kubernetes and SQL. All behaviour lives in `agent-cli-core`; this crate is
//! the list of domains and the tests that hold the whole registry to the rules.

use std::process::ExitCode;

use agent_cli_core::Domain;

/// Every domain, in overview order. Each later phase adds its crate's `DOMAIN`.
const DOMAINS: &[Domain] = &[
    agent_cli_ado::DOMAIN,
    agent_cli_azure::KV,
    agent_cli_azure::ACR,
    agent_cli_azure::AKS,
    agent_cli_k8s::K8S,
    agent_cli_sql::DOMAIN,
    agent_cli_airflow::DOMAIN,
];

fn main() -> ExitCode {
    agent_cli_core::run(DOMAINS)
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{
        FakeTransport, assert_read_only_refuses, assert_search_quality, run,
    };

    use super::DOMAINS;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(
            agent_cli_core::check_registry(DOMAINS),
            Vec::<String>::new()
        );
    }

    #[test]
    fn search_finds_the_labeled_command() {
        assert_search_quality(DOMAINS, include_str!("../tests/search.toml"));
    }

    #[test]
    fn read_only_mode_refuses_every_change() {
        assert_read_only_refuses(DOMAINS);
    }

    /// Every section set, with names as long as real ones get: the overview
    /// still fits in 1 KB (check_registry also holds it at the cap for every
    /// domain), and each domain's status line says something.
    #[test]
    fn the_overview_fits_with_every_domain_configured() {
        let config = r#"
[ado]
org = "contoso-engineering-platform"
project = "Fabrikam Fiber Commerce"

[azure]
vaults = ["kv-contoso-prod-westeurope", "kv-contoso-staging-westeurope"]
registries = "contosoacr"

[[k8s.scope]]
name = "prod"
context = "aks-contoso-prod-westeurope"
namespaces = ["web", "jobs"]

[[k8s.scope]]
name = "staging"

[[sql.connection]]
name = "reporting"
kind = "mssql"
host = "sql-contoso-reporting.database.windows.net"
database = "reporting"
user = "reader"
password_env = "REPORTING_PASSWORD"

[[sql.connection]]
name = "ledger"
kind = "oracle"
host = "ledger.contoso.example"
service = "LEDGER"
user = "reader"
password_cmd = "pass show contoso/ledger"

[[airflow.instance]]
name = "dev"
base_url = "https://airflow-dev.contoso.example"
token_env = "AIRFLOW_DEV_TOKEN"

[[airflow.instance]]
name = "prod"
base_url = "https://airflow.contoso.example"
username = "agent"
password_env = "AIRFLOW_PROD_PASSWORD"
read_only = true
k8s_scope = "prod"
"#;
        let setup = Setup::fake(FakeTransport::default()).with_config(config);
        let outcome = run(DOMAINS, &[], setup);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome.stdout.len() <= 1024,
            "{} bytes:\n{}",
            outcome.stdout.len(),
            outcome.stdout
        );
        let config_line = outcome
            .stdout
            .lines()
            .find(|line| line.starts_with("Config:"))
            .unwrap();
        for status in [
            "ado contoso-engineeri\u{2026}",
            "kv 2 vaults",
            "acr 1 registry",
            "k8s 2 scopes",
            "sql 2 connections",
            "airflow 2 instances",
        ] {
            assert!(config_line.contains(status), "{status}: {config_line}");
        }
    }
}
