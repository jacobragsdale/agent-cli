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
    agent_cli_dd::DOMAIN,
];

fn main() -> ExitCode {
    agent_cli_core::run(DOMAINS)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use agent_cli_core::testing::{
        FakeTransport, assert_read_only_refuses, assert_search_quality, printed_command_problems,
        run,
    };
    use agent_cli_core::{BUILTINS, Effect, Setup, command_help};

    use super::DOMAINS;

    const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    /// `docs/reference/`, rendered from the registry: `README.md`, the index,
    /// and one `<domain>.md` per domain holding every command's help, as
    /// `agent-cli <path> --help` prints it. Pairs of file name and text.
    fn reference() -> Vec<(String, String)> {
        let total: usize = DOMAINS.iter().map(|domain| domain.commands.len()).sum();
        let mut index = format!(
            "# Command reference\n\n\
             Generated from the registry by `crates/cli` (`UPDATE_DOCS=1 cargo test -p agent-cli \
             reference`); a test fails when it is stale. Each domain's page holds, for every \
             command, what `agent-cli <domain> <resource> <verb> --help` prints: arguments (`*` \
             required), `Returns:`, the effect, and an example.\n\n\
             {total} commands in {} domains. Every command also takes the globals `--fields a,b.c`, \
             `--raw`, `--dry-run`, `--yes`, `--reveal`, `--timeout S`, `--output FILE` and \
             `--no-cache`. Exit codes: 0 ok, 1 failed, 2 fix the call, 3 needs setup, 4 not found, \
             5 conflict, 124 timed out.\n\n\
             | Domain | Summary | Commands |\n|---|---|---|\n",
            DOMAINS.len()
        );
        let mut files = Vec::new();
        for domain in DOMAINS {
            index.push_str(&format!(
                "| [{0}]({0}.md) | {1} | {2} |\n",
                domain.name,
                domain.summary,
                domain.commands.len()
            ));
            let mut out = format!(
                "# {} \u{2014} {} ({} commands)\n\n| Command | Effect | Summary |\n|---|---|---|\n",
                domain.name,
                domain.summary,
                domain.commands.len()
            );
            for command in domain.commands {
                let path = command.path.join(" ");
                let effect = match command.effect {
                    Effect::Read => "read",
                    Effect::Write => "write",
                    Effect::Destructive => "destructive",
                    Effect::Reveal => "reveal",
                    Effect::Varies => "read or write",
                };
                out.push_str(&format!(
                    "| [`{path}`](#{}) | {effect} | {} |\n",
                    path.replace(' ', "-"),
                    command.summary.replace('|', "\\|")
                ));
            }
            for command in domain.commands {
                out.push_str(&format!(
                    "\n### {}\n\n```text\n{}\n```\n",
                    command.path.join(" "),
                    command_help(command)
                ));
            }
            files.push((format!("{}.md", domain.name), out));
        }
        files.push(("README.md".to_owned(), index));
        files
    }

    /// Every page is fresh, and no page in `docs/reference/` is one the
    /// registry no longer produces. `UPDATE_DOCS` rewrites them and removes
    /// the strays.
    #[test]
    fn the_command_reference_matches_the_registry() {
        let dir = Path::new(REPO).join("docs/reference");
        let fresh = reference();
        let update = std::env::var_os("UPDATE_DOCS").is_some();
        let mut stale = Vec::new();
        for (name, text) in &fresh {
            let path = dir.join(name);
            if update {
                std::fs::write(&path, text).unwrap();
            }
            if std::fs::read_to_string(&path).unwrap_or_default() != *text {
                stale.push(name.clone());
            }
        }
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".md") && !fresh.iter().any(|(fresh, _)| *fresh == name) {
                if update {
                    std::fs::remove_file(entry.path()).unwrap();
                } else {
                    stale.push(format!("{name} (no domain produces it)"));
                }
            }
        }
        assert!(
            stale.is_empty(),
            "docs/reference is stale: {stale:?}; run UPDATE_DOCS=1 cargo test -p agent-cli reference"
        );
    }

    /// The Markdown a reader follows: README, AGENTS.md, docs/ and the
    /// world's README and facts. Not the plans, which name commands not built yet, nor
    /// the generated reference, whose examples `check_registry` parses.
    fn documents() -> Vec<PathBuf> {
        fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() && !path.ends_with("plans") && !path.ends_with("reference") {
                    walk(&path, found);
                } else if path.extension().is_some_and(|ext| ext == "md") {
                    found.push(path);
                }
            }
        }
        let repo = Path::new(REPO);
        let mut found = vec![repo.join("README.md"), repo.join("AGENTS.md")];
        walk(&repo.join("docs"), &mut found);
        walk(&repo.join("fixtures/world"), &mut found);
        found
    }

    /// Every `agent-cli …` call in a `sh` block of the docs names a real
    /// domain and parses against the registry, and so does every one in the
    /// prose around the blocks. Other blocks are not calls: `text` is output,
    /// and a `shell` block holds a call that fails on purpose.
    #[test]
    fn every_command_line_in_the_docs_parses() {
        let mut problems = Vec::new();
        for path in documents() {
            let text = std::fs::read_to_string(&path).unwrap();
            let name = path
                .strip_prefix(REPO)
                .unwrap_or(&path)
                .display()
                .to_string();
            let mut report = |found: Vec<String>| {
                problems.extend(
                    found
                        .into_iter()
                        .map(|problem| format!("{name}: {problem}")),
                );
            };
            let mut fence: Option<String> = None;
            let mut prose = String::new();
            for line in text.lines() {
                if let Some(language) = line.trim_start().strip_prefix("```") {
                    fence = match fence {
                        Some(_) => None,
                        None => Some(language.trim().to_owned()),
                    };
                    continue;
                }
                match fence.as_deref() {
                    None => {
                        prose.push_str(line);
                        prose.push('\n');
                    }
                    Some("sh") => {
                        let call = line.trim().trim_start_matches("$ ");
                        let Some(rest) = call.strip_prefix("agent-cli") else {
                            continue;
                        };
                        if !rest.is_empty() && !rest.starts_with(' ') {
                            continue;
                        }
                        let first = rest
                            .split("  ")
                            .next()
                            .and_then(|words| words.split_whitespace().next())
                            .unwrap_or_default();
                        let known = first.is_empty()
                            || first.starts_with('-')
                            || BUILTINS.contains(&first)
                            || DOMAINS.iter().any(|domain| domain.name == first);
                        if !known {
                            report(vec![format!("`{call}` names no domain")]);
                        }
                        report(printed_command_problems(DOMAINS, call));
                    }
                    Some(_) => {}
                }
            }
            report(printed_command_problems(DOMAINS, &prose));
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(
            agent_cli_core::check_registry(DOMAINS),
            Vec::<String>::new()
        );
    }

    /// Each crate keeps its own labeled queries in `crates/<crate>/search.toml`,
    /// so a new crate's file counts with no edit here.
    #[test]
    fn search_finds_the_labeled_command() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(Path::new(REPO).join("crates"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path().join("search.toml"))
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        let queries: String = files
            .iter()
            .map(|path| std::fs::read_to_string(path).unwrap() + "\n")
            .collect();
        assert_search_quality(DOMAINS, &queries);
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
[datadog]
site = "datadoghq.eu"
env = "prod"
token_cmd = "pup auth token"
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
            "dd eu",
        ] {
            assert!(config_line.contains(status), "{status}: {config_line}");
        }
    }
}
