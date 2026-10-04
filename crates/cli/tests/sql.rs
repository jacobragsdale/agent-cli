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

/// `config.test.toml` and a SYSTEM login on the Oracle container (the
/// throwaway password is `compose.yaml`'s), which may read `v$session`.
fn watching_config(dir: &tempfile::TempDir) -> String {
    let path = dir.path().join("config.toml");
    let config = std::fs::read_to_string(test_config()).unwrap()
        + "\n[[sql.connection]]\nname = \"oracle-system\"\nkind = \"oracle\"\n\
           host = \"localhost\"\nport = 1521\nservice = \"FREEPDB1\"\nuser = \"system\"\n\
           password = \"Bench_Pass1!\"\n";
    std::fs::write(&path, config).unwrap();
    path.to_str().unwrap().to_owned()
}

/// The first cell of a one-row query, as a whole number or text.
fn first_cell(config: &str, conn: &str, sql: &str) -> serde_json::Value {
    let output = agent_cli(config, &["sql", "query", "run", "--conn", conn, sql], &[]);
    assert!(output.status.success(), "{output:?}");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    result["results"][0]["rows"][0][0].clone()
}

/// Runs `args`, signals it (`signal`, or none: its own `--timeout`) once
/// `running(pid)` counts its statement on the server, and waits for the
/// count to reach zero. The exit code, stderr, and how long the server
/// went on after the signal or deadline.
fn stop_midway(
    config: &str,
    args: &[&str],
    signal: Option<&str>,
    running: impl Fn(u32) -> i64,
) -> (Option<i32>, String, std::time::Duration) {
    let started = Instant::now();
    let child = Command::new(env!("CARGO_BIN_EXE_agent-cli"))
        .args(args)
        .env("AGENT_CLI_CONFIG", config)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    while running(pid) == 0 {
        assert!(
            started.elapsed().as_secs() < 20,
            "the statement never started"
        );
    }
    let stopped = match signal {
        Some(signal) => {
            let stopped = Instant::now();
            let sent = Command::new("kill")
                .args([format!("-{signal}"), pid.to_string()])
                .status()
                .unwrap();
            assert!(sent.success());
            stopped
        }
        None => started,
    };
    let output = child.wait_with_output().unwrap();
    while running(pid) > 0 {
        assert!(stopped.elapsed().as_secs() < 60, "the server never stopped");
    }
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (output.status.code(), stderr, stopped.elapsed())
}

/// Ctrl-C mid-transaction ends the request on the server at once, and what
/// it wrote is rolled back.
#[test]
fn mssql_a_signal_ends_the_request_and_rolls_back_its_write() {
    if !wanted() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let config = watching_config(&dir);
    let sql = "begin tran; update bench.customers set name = 'interrupted' where id = 50; \
               waitfor delay '00:01:00'; commit -- signal test";
    let running = |_| {
        first_cell(
            &config,
            "local-mssql",
            "select count(*) from sys.dm_exec_requests r \
             cross apply sys.dm_exec_sql_text(r.sql_handle) t \
             where t.text like '%-- signal test' and r.session_id <> @@spid",
        )
        .as_i64()
        .unwrap()
    };
    let args = ["sql", "query", "run", "--conn", "local-mssql", sql, "--yes"];
    let (code, stderr, after) = stop_midway(&config, &args, Some("INT"), running);
    assert_eq!(code, Some(130), "{stderr}");
    assert_eq!(
        stderr,
        "agent-cli: SIGINT: stopped the session on local-mssql\n"
    );
    assert!(
        after.as_secs() < 3,
        "the request ran {after:?} past the signal"
    );
    let name = first_cell(
        &config,
        "local-mssql",
        "select name from bench.customers where id = 50",
    );
    assert_ne!(name, "interrupted");
}

/// A signal, and the deadline, each break a statement the server is busy
/// with, and the session is gone within moments.
#[test]
fn oracle_a_signal_and_the_deadline_each_end_the_statement_on_the_server() {
    if !wanted() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let config = watching_config(&dir);
    let busy = "select count(*) from (select level l from dual connect by level <= 100000) a, \
                (select level l from dual connect by level <= 100000) b";
    let running = |pid: u32| {
        first_cell(
            &config,
            "oracle-system",
            &format!(
                "select count(*) from v$session where username = 'BENCH' \
                 and status = 'ACTIVE' and process = '{pid}'"
            ),
        )
        .as_i64()
        .unwrap()
    };
    let args = ["sql", "query", "run", "--conn", "local-oracle", busy];
    let (code, stderr, after) = stop_midway(&config, &args, Some("TERM"), running);
    assert_eq!(code, Some(143), "{stderr}");
    assert_eq!(
        stderr,
        "agent-cli: SIGTERM: stopped the session on local-oracle\n"
    );
    assert!(
        after.as_secs() < 3,
        "the statement ran {after:?} past the signal"
    );

    let timed = [&args[..], &["--timeout", "4"]].concat();
    let (code, stderr, after) = stop_midway(&config, &timed, None, running);
    assert_eq!(code, Some(124), "{stderr}");
    assert!(
        after.as_secs() < 7,
        "the statement ran {after:?} past its start"
    );
}
