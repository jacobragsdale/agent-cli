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
