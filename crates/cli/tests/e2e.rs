//! The built binary, as an agent runs it.

use std::process::Command;

#[test]
fn no_args_prints_the_overview_and_exits_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-cli"))
        .env("AGENT_CLI_CONFIG", "/nonexistent/agent-cli/config.toml")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let overview = String::from_utf8(output.stdout).unwrap();
    assert!(overview.len() < 1024, "{} bytes", overview.len());
    assert!(overview.starts_with("agent-cli: "), "{overview}");
}

#[test]
fn an_unknown_word_exits_two_with_the_error_on_stderr() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-cli"))
        .args(["nope", "thing", "list"])
        .env("AGENT_CLI_CONFIG", "/nonexistent/agent-cli/config.toml")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("error: unknown domain \"nope\""));
}

/// Whether `pid` is gone (a zombie awaiting its reaper counts as gone).
#[cfg(unix)]
fn gone(pid: &str) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    stat.trim().is_empty() || stat.trim().starts_with('Z')
}

/// A terminating signal kills the child agent-cli started, which leads a
/// process group of its own that the terminal's signal never reaches, says
/// so on stderr and exits 128 + the signal.
#[test]
#[cfg(unix)]
fn a_signal_kills_the_child_process_and_exits_128_plus_it() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[[sql.connection]]\nname = \"slow\"\nkind = \"mssql\"\nhost = \"127.0.0.1\"\n\
             port = 9\ndatabase = \"bench\"\nuser = \"sa\"\npassword_cmd = \"echo $$ > {}; sleep 30\"\n",
            pid_file.display()
        ),
    )
    .unwrap();
    for (signal, code) in [("TERM", 143), ("INT", 130)] {
        let _ = std::fs::remove_file(&pid_file);
        let child = Command::new(env!("CARGO_BIN_EXE_agent-cli"))
            .args(["sql", "query", "run", "select 1"])
            .env("AGENT_CLI_CONFIG", &config)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        let password_cmd = loop {
            match std::fs::read_to_string(&pid_file) {
                Ok(pid) if pid.ends_with('\n') => break pid.trim().to_owned(),
                _ if started.elapsed().as_secs() > 10 => panic!("password_cmd never started"),
                _ => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        };
        // The handler is in place before any child starts.
        let killed = Command::new("kill")
            .args([format!("-{signal}"), child.id().to_string()])
            .status()
            .unwrap();
        assert!(killed.success());
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!("agent-cli: SIG{signal}: stopped sh (pid {password_cmd})\n")
        );
        assert!(
            gone(&password_cmd),
            "the password_cmd {password_cmd} survived"
        );
    }
}
