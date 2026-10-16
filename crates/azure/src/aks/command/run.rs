//! `aks command run`: `az aks command invoke` without az. ARM's `runCommand`
//! starts a pod inside the cluster that runs the words after `--` (kubectl
//! and helm are in its image), so it reaches a private API server that this
//! machine, such as a WSL shell off the VPN, cannot. The answer is polled
//! from `commandResults` until it ends or the deadline does.

use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Effect, Exit, Failure, Method, Request, command};
use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::aks::{Place, arm_only, locate};
use crate::client::{ARM, bearer, stamp, text};
use crate::config::Azure;

/// What a poll waits when ARM sends no `Retry-After`.
const POLL: Duration = Duration::from_secs(5);
/// What the wait leaves of the deadline for its last poll and its answer.
const MARGIN: Duration = Duration::from_secs(2);

#[derive(clap::Args)]
pub struct CommandRunArgs {
    /// The cluster: its name as `aks cluster list` shows it, its ARM id, or its portal link
    cluster: String,
    #[command(flatten)]
    place: Place,
    /// How many of the output's last lines to keep
    #[arg(long, default_value_t = 200)]
    tail: usize,
    /// The command after `--`, such as kubectl get pods -A
    #[arg(last = true, required = true)]
    argv: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CommandResult {
    /// The command's id in AKS.
    id: Option<String>,
    cluster: String,
    command: String,
    /// succeeded or failed: whether it ran, not how it ended.
    state: Option<String>,
    exit_code: Option<i64>,
    /// Why it could not run, when it failed to.
    reason: Option<String>,
    started: Option<String>,
    finished: Option<String>,
    /// The last `--tail` lines of its output.
    logs: Option<String>,
}

/// The words as one shell line: AKS takes a string.
// VERIFY(work): that AKS runs `command` through a shell, so quotes matter.
fn shell_line(argv: &[String]) -> String {
    let plain = |word: &str| {
        !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c))
    };
    argv.iter()
        .map(|word| {
            if plain(word) {
                word.clone()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a result is the end: it ran (succeeded) or could not (failed).
fn ended(result: &Value) -> bool {
    let state = text(&result["properties"]["provisioningState"]).unwrap_or_default();
    ["succeeded", "failed", "canceled"]
        .iter()
        .any(|end| state.eq_ignore_ascii_case(end))
}

fn command_run(ctx: &Ctx, args: CommandRunArgs) -> Result<CommandResult> {
    let azure = Azure::load(ctx)?;
    let (located, _) = locate(ctx, &azure, &args.cluster, &args.place)?;
    let line = shell_line(&args.argv);
    // VERIFY(work): whether a cluster with local accounts off needs
    // `clusterToken` (az sends one for the AKS AAD server app) or takes the
    // ARM token's RBAC alone.
    let mint = bearer(ctx, ARM);
    let request = Request::new(Method::Post, located.url("/runCommand"))
        .json(json!({ "command": line }))
        .auth(&mint);
    // VERIFY(work): a 202 with a Location of
    // `…/commandResults/ID?api-version=…`, which answers 202 while the pod
    // runs and 200 with the result after, as the REST reference shows.
    let mut answer = ctx.write(Effect::Destructive, request)?;
    let mut location = answer.header("location").map(str::to_owned);
    let begun = Instant::now();
    let result = loop {
        if answer.status != 202 {
            let result = answer.json()?;
            if ended(&result) {
                break result;
            }
        }
        let Some(poll) = &location else {
            bail!(
                "ARM answered runCommand with {} and no Location to poll",
                answer.status
            );
        };
        let left = ctx.deadline().saturating_duration_since(Instant::now());
        if left <= MARGIN {
            return Err(Failure::timed_out(format!(
                "`{line}` on {} has not ended after {}s; it keeps running in the cluster",
                located.name,
                begun.elapsed().as_secs()
            ))
            .hint("give it longer with --timeout SECONDS, after checking what it already changed")
            .into());
        }
        let wait = answer
            .header("retry-after")
            .and_then(|seconds| seconds.trim().parse().ok())
            .map_or(POLL, |seconds: u64| Duration::from_secs(seconds.max(1)));
        std::thread::sleep(wait.min(left - MARGIN));
        answer = ctx.read(Request::get(arm_only(poll)?).auth(&mint))?;
        if let Some(next) = answer.header("location") {
            location = Some(next.to_owned());
        }
    };
    let props = &result["properties"];
    // VERIFY(work): how long an output AKS keeps, and whether it cuts the
    // head or the tail; this keeps the last --tail lines of what it sends.
    let logs = text(&props["logs"]).map(|logs| {
        let lines: Vec<&str> = logs.lines().collect();
        if lines.len() > args.tail {
            ctx.note(format!(
                "[last {} of {} lines; --tail N]",
                args.tail,
                lines.len()
            ));
        }
        lines[lines.len().saturating_sub(args.tail)..].join("\n")
    });
    let row = CommandResult {
        id: text(&result["id"]),
        cluster: located.name,
        command: line,
        state: text(&props["provisioningState"]),
        exit_code: props["exitCode"].as_i64(),
        reason: text(&props["reason"]),
        started: stamp(&props["startedAt"]),
        finished: stamp(&props["finishedAt"]),
        logs,
    };
    if row
        .state
        .as_deref()
        .is_some_and(|state| !state.eq_ignore_ascii_case("succeeded"))
    {
        let why = row.reason.as_deref().unwrap_or("AKS gave no reason");
        let message = format!("`{}` could not run on {}: {why}", row.command, row.cluster);
        return Err(Failure::new(Exit::Failed, message).with_data(&row).into());
    }
    if let Some(code) = row.exit_code.filter(|code| *code != 0) {
        let message = format!("`{}` exited {code}", row.command);
        return Err(Failure::new(Exit::Failed, message).with_data(&row).into());
    }
    Ok(row)
}

command! {
    pub COMMAND_RUN = ["aks", "command", "run"], Destructive,
    "Run kubectl or helm inside an AKS cluster through ARM, for a private API server",
    keywords: ["invoke", "kubectl", "helm", "private", "unreachable", "vpn", "runcommand", "exec", "shell"],
    example: "aks command run aks-contoso-prod --yes -- kubectl get pods -A",
    timeout: 100,
    run: command_run,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, assert_dry_run};
    use serde_json::json;

    use super::shell_line;
    use crate::AKS;
    use crate::testing::{AKS_ID, azure};

    fn result(state: &str, code: i64, logs: &str) -> Answer {
        Answer::json(
            &json!({"id": "0e9872e6629349dc865e27ee6f8bab2d", "properties": {
            "provisioningState": state, "exitCode": code, "logs": logs,
            "startedAt": "2026-10-01T10:00:00Z", "finishedAt": "2026-10-01T10:00:04Z"}}),
        )
    }

    fn argv(extra: &[&str]) -> Vec<String> {
        let mut argv: Vec<String> = [
            "aks", "command", "run", AKS_ID, "--yes", "--tail", "2", "--",
        ]
        .iter()
        .map(|word| (*word).to_owned())
        .collect();
        argv.extend(extra.iter().map(|word| (*word).to_owned()));
        argv
    }

    #[test]
    fn the_words_after_the_dashes_become_one_shell_line() {
        let words = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            shell_line(&words(&[
                "kubectl",
                "get",
                "pods",
                "-l",
                "app=api",
                "-o",
                "jsonpath={.items[*].metadata.name}"
            ])),
            "kubectl get pods -l app=api -o 'jsonpath={.items[*].metadata.name}'"
        );
        assert_eq!(shell_line(&words(&["echo", "it's"])), r"echo 'it'\''s'");
    }

    #[test]
    fn run_posts_the_command_then_polls_its_result_and_keeps_the_tail() {
        let location = format!(
            "https://management.azure.com{AKS_ID}/commandResults/0e9872e6629349dc865e27ee6f8bab2d?api-version=2026-07-01"
        );
        let argv = argv(&["kubectl", "get", "nodes"]);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let (outcome, transport) = azure(
            &[AKS],
            &argv,
            vec![
                Answer::status(202, "")
                    .with_header("Location", &location)
                    .with_header("Retry-After", "1"),
                Answer::status(202, "").with_header("Location", &location),
                result("succeeded", 0, "NAME STATUS\nnode-0 Ready\nnode-1 Ready"),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["command"], "kubectl get nodes");
        assert_eq!(got["logs"], "node-0 Ready\nnode-1 Ready");
        assert_eq!(got["exit_code"], 0);
        assert!(
            outcome.stderr.contains("[last 2 of 3 lines; --tail N]"),
            "{}",
            outcome.stderr
        );
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            format!("https://management.azure.com{AKS_ID}/runCommand?api-version=2026-07-01")
        );
        assert_eq!(sent[0].body, Some(json!({"command": "kubectl get nodes"})));
        assert_eq!(sent[2].url, location);
    }

    #[test]
    fn a_failing_command_prints_its_result_and_exits_1() {
        let argv = argv(&["kubectl", "get", "pods", "-n", "nope"]);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let (outcome, _) = azure(
            &[AKS],
            &argv,
            vec![result(
                "succeeded",
                1,
                "No resources found in nope namespace.",
            )],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(outcome.json()["exit_code"], 1);
        assert!(outcome.stderr.contains("exited 1"), "{}", outcome.stderr);
    }

    #[test]
    fn a_location_off_arm_gets_no_token() {
        let argv = argv(&["kubectl", "version"]);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let (outcome, transport) = azure(
            &[AKS],
            &argv,
            vec![
                Answer::status(202, "")
                    .with_header("Location", "https://example.com/steal")
                    .with_header("Retry-After", "1"),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(transport.sent().len(), 1);
    }

    #[test]
    fn a_dry_run_plans_the_post_and_sends_nothing() {
        let plans = assert_dry_run(
            &[AKS],
            &[
                "aks", "command", "run", AKS_ID, "--", "kubectl", "get", "pods",
            ],
            vec![],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(plans[0]["body"], json!({"command": "kubectl get pods"}));
    }
}
