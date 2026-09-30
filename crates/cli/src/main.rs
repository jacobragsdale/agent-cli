//! `agent-cli`: one CLI that gives coding agents Azure DevOps, Azure,
//! Kubernetes and SQL. All behaviour lives in `agent-cli-core`; this crate is
//! the list of domains and the tests that hold the whole registry to the rules.

use std::process::ExitCode;

use agent_cli_core::Domain;

/// Every domain, in overview order. Each later phase adds its crate's `DOMAIN`.
const DOMAINS: &[Domain] = &[
    agent_cli_azure::KV,
    agent_cli_azure::ACR,
    agent_cli_azure::AKS,
    agent_cli_k8s::K8S,
    agent_cli_sql::DOMAIN,
];

fn main() -> ExitCode {
    agent_cli_core::run(DOMAINS)
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{assert_read_only_refuses, assert_search_quality};

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
}
