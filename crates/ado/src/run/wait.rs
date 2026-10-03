use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Exit, Failure, Method, command};
use anyhow::Result;

use crate::client::{Ado, Body, Kind, rate_limit_pause};

use super::{Gate, RunIdArgs, RunRow, build_url, gate, new_run, run_row, timeline};

/// How often `run wait` asks, which is how often ticket-tui's watcher did.
const POLL: Duration = Duration::from_secs(15);

/// What a wait leaves of the deadline for its last poll and its answer.
const MARGIN: Duration = Duration::from_secs(2);

fn run_wait(ctx: &Ctx, args: RunIdArgs) -> Result<RunRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.id)?;
    let url = build_url(&ado, id);
    let started = Instant::now();
    loop {
        let response = match ado.send(ctx, Method::Get, &url, Body::None, None) {
            Ok(response) => response,
            // A dropped connection is not the run failing: ask again until
            // the deadline, and say so there.
            Err(error) if error.downcast_ref::<Failure>().is_none() => {
                let left = ctx.deadline().saturating_duration_since(Instant::now());
                if left <= MARGIN {
                    return Err(Failure::timed_out(format!(
                        "Azure DevOps did not answer about run {id} ({error:#})"
                    ))
                    .hint(format!("agent-cli ado run wait {id}"))
                    .into());
                }
                std::thread::sleep(POLL.min(left - MARGIN));
                continue;
            }
            Err(error) => return Err(error),
        };
        let run = run_row(&response.json()?);
        if run.status.as_deref() == Some("completed") {
            if run.result.as_deref() == Some("succeeded") {
                return Ok(run);
            }
            return Err(Failure::new(
                Exit::Failed,
                format!(
                    "run {id} finished {}",
                    run.result.as_deref().unwrap_or("without a result")
                ),
            )
            .hint(match run.result.as_deref() {
                Some("canceled") => format!("{}  (a new run)", new_run(&run)),
                Some("partiallySucceeded") => format!("agent-cli ado run logs {id}"),
                _ => format!(
                    "agent-cli ado run get {id} --fields failed, then agent-cli ado run logs {id}"
                ),
            })
            .with_data(run)
            .into());
        }
        let left = ctx.deadline().saturating_duration_since(Instant::now());
        if left <= MARGIN {
            // A run parked at a gate waits on a person, not time.
            let gate = timeline(ctx, &ado, id)
                .ok()
                .and_then(|records| gate(&records));
            let failure = match gate {
                Some(Gate::Approval(approval)) => Failure::timed_out(format!(
                    "run {id} waits on approval {approval}, which nobody has answered"
                ))
                .hint("agent-cli ado approval list  (who can answer it; waiting will not move the run)"),
                Some(Gate::Permission) => Failure::timed_out(format!(
                    "run {id} waits for a person to permit a resource it uses (an agent pool, an environment or a repository)"
                ))
                .hint(format!(
                    "Permit it on the run's page, {}; waiting will not move the run",
                    run.url.as_deref().unwrap_or("in Azure DevOps")
                )),
                None => Failure::timed_out(format!(
                    "run {id} is still {} after {}s",
                    run.status.as_deref().unwrap_or("going"),
                    started.elapsed().as_secs()
                ))
                .hint(format!(
                    "run it again to keep waiting (it stays under your shell's 2-minute limit): agent-cli ado run wait {id}"
                )),
            };
            return Err(failure.with_data(run).into());
        }
        // A spent rate-limit budget asks for longer than the usual poll.
        let pause = rate_limit_pause(&response).unwrap_or(POLL);
        std::thread::sleep(pause.min(left - MARGIN));
    }
}

command! {
    pub RUN_WAIT = ["ado", "run", "wait"], Read,
    "Wait for a run to finish: exit 0 if it succeeded, 1 if not, 124 if still going",
    keywords: ["until", "done", "finish", "complete", "block", "poll", "watch", "build"],
    example: "ado run wait 1234 --fields id,status,result",
    timeout: 100,
    run: run_wait,
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use agent_cli_core::testing::Answer;

    use crate::testing::{ado, build};

    #[test]
    fn wait_exits_0_when_it_succeeded_and_1_with_the_state_when_it_did_not() {
        let (outcome, _) = ado(
            &["ado", "run", "wait", "991"],
            vec![Answer::json(&build(991, "completed", Some("succeeded")))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["result"], "succeeded");

        let (outcome, _) = ado(
            &["ado", "run", "wait", "991"],
            vec![Answer::json(&build(991, "completed", Some("failed")))],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(outcome.json()["result"], "failed", "the run it waited on");
        assert!(
            outcome
                .stderr
                .starts_with("error: run 991 finished failed\n"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli ado run get 991 --fields failed"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_wait_on_a_run_parked_at_an_approval_says_so_at_the_deadline() {
        let gate = serde_json::json!({"records": [
            {"type": "Checkpoint.Approval", "id": "fe353cd7-2b39-4ba4-ab3d-d82379827795",
             "state": "inProgress"}
        ]});
        let (outcome, _) = ado(
            &["ado", "run", "wait", "991", "--timeout", "3"],
            vec![
                Answer::json(&build(991, "inProgress", None)),
                Answer::json(&build(991, "inProgress", None)),
                Answer::json(&gate),
            ],
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert!(
            outcome.stderr.contains(
                "error: run 991 waits on approval fe353cd7-2b39-4ba4-ab3d-d82379827795, which nobody has answered\nhint: agent-cli ado approval list  ("
            ),
            "{}",
            outcome.stderr
        );
        assert_eq!(outcome.json()["status"], "inProgress");
    }

    #[test]
    fn a_wait_on_a_run_waiting_for_permission_says_so_at_the_deadline() {
        let gate = serde_json::json!({"records": [
            {"type": "Checkpoint.Authorization", "state": "inProgress"}
        ]});
        let (outcome, _) = ado(
            &["ado", "run", "wait", "991", "--timeout", "3"],
            vec![
                Answer::json(&build(991, "notStarted", None)),
                Answer::json(&build(991, "notStarted", None)),
                Answer::json(&gate),
            ],
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("error: run 991 waits for a person to permit a resource it uses"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("hint: Permit it on the run's page"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn wait_at_the_deadline_is_124_with_the_current_state() {
        let started = Instant::now();
        let (outcome, transport) = ado(
            &["ado", "run", "wait", "991", "--timeout", "3"],
            vec![
                Answer::json(&build(991, "inProgress", None)),
                Answer::json(&build(991, "inProgress", None)),
                Answer::ok(""),
            ],
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert_eq!(
            transport.sent().len(),
            3,
            "polled, slept to the margin, polled once more, then read what it waits on"
        );
        assert!(
            outcome.stderr.contains("run 991 is still inProgress after"),
            "{}",
            outcome.stderr
        );
        assert_eq!(outcome.json()["status"], "inProgress", "its current state");
        assert!(
            outcome
                .stderr
                .contains("to keep waiting (it stays under your shell's 2-minute limit): agent-cli ado run wait 991\n"),
            "{}",
            outcome.stderr
        );
    }
}
