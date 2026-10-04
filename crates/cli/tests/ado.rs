//! The ado domain through the built binary, with a placeholder config and no
//! credential: an empty `AZURE_CONFIG_DIR` is an Azure CLI nobody signed in
//! to, so nothing here reaches Azure DevOps.

use std::process::{Command, Output};

fn agent_cli(args: &[&str]) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\n",
    )
    .unwrap();
    Command::new(env!("CARGO_BIN_EXE_agent-cli"))
        .args(args)
        .env("AGENT_CLI_CONFIG", &config)
        .env("AZURE_CONFIG_DIR", dir.path())
        .env("XDG_CACHE_HOME", dir.path())
        .env_remove("AZURE_DEVOPS_EXT_PAT")
        .output()
        .unwrap()
}

#[test]
fn a_write_that_reads_nothing_first_plans_without_a_credential() {
    let output = agent_cli(&["ado", "run", "cancel", "991", "--dry-run"]);
    assert!(output.status.success(), "{output:?}");
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["would"][0]["method"], "PATCH");
    assert_eq!(
        plan["would"][0]["url"],
        "https://dev.azure.com/contoso/Fabrikam/_apis/build/builds/991?api-version=7.1"
    );
}

#[test]
fn a_read_without_a_credential_is_needs_setup_naming_both_ways_in() {
    let output = agent_cli(&["ado", "workitem", "get", "42"]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("hint: set pat_env or pat_cmd under [ado] (or AZURE_DEVOPS_EXT_PAT)"),
        "{stderr}"
    );
}
