//! The sql domain through the built binary, as an agent runs it.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::time::Instant;

fn agent_cli(config: &str, args: &[&str], envs: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-cli"))
        .args(args)
        .env("AGENT_CLI_CONFIG", config)
        .envs(envs.iter().copied())
        .output()
        .unwrap()
}

/// The binary starts, and every command that needs no Oracle works, on a
/// machine without Instant Client; an Oracle connection then fails with
/// exit 3 and says what to set. A process of its own, as ODPI-C loads the
/// client once per process.
#[test]
fn without_instant_client_oracle_fails_cleanly_and_the_rest_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        "[[sql.connection]]\nname = \"ledger\"\nkind = \"oracle\"\nhost = \"localhost\"\n\
         service = \"FREEPDB1\"\nuser = \"bench\"\npassword = \"bench\"\n",
    )
    .unwrap();
    let config = config.to_str().unwrap();
    let client = [("AGENT_CLI_SQL_ORACLE_CLIENT_DIR", empty.to_str().unwrap())];

    let listed = agent_cli(config, &["sql", "connection", "list"], &client);
    assert!(listed.status.success(), "{listed:?}");
    assert!(String::from_utf8_lossy(&listed.stdout).contains("\"ledger\""));

    let failed = agent_cli(
        config,
        &[
            "sql",
            "query",
            "run",
            "--conn",
            "ledger",
            "select 1 from dual",
        ],
        &client,
    );
    assert_eq!(failed.status.code(), Some(3), "{failed:?}");
    assert!(failed.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(
        stderr.starts_with(&format!(
            "error: Oracle client library not found in {}",
            empty.display()
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains("hint: set [sql] oracle_client_dir or AGENT_CLI_SQL_ORACLE_CLIENT_DIR"),
        "{stderr}"
    );
}

fn test_config() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../config.test.toml").to_owned()
}

fn wanted() -> bool {
    let on = std::env::var("AGENT_CLI_TEST_DBS").is_ok_and(|value| value == "1");
    if !on {
        eprintln!("skipped: set AGENT_CLI_TEST_DBS=1 with scripts/db-up.sh's databases up");
    }
    on
}

#[test]
fn mssql_select_one_through_the_binary_and_sql_from_stdin() {
    if !wanted() {
        return;
    }
    let started = Instant::now();
    let output = agent_cli(
        &test_config(),
        &[
            "sql",
            "query",
            "run",
            "--conn",
            "local-mssql",
            "select 1 as one",
        ],
        &[],
    );
    eprintln!("select 1 end to end: {:?}", started.elapsed());
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["results"][0]["rows"], serde_json::json!([[1]]));

    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-cli"))
        .args(["sql", "query", "run", "--conn", "local-mssql", "-"])
        .env("AGENT_CLI_CONFIG", test_config())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"select 2 as two\ngo\nselect 3 as three\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["results"][1]["rows"], serde_json::json!([[3]]));
}
