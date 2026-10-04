use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Exit, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Airflow, seconds, text};

use super::{RunIdArgs, failed_ids};

/// How `run wait` starts polling, and the most it lets a poll wait.
const FIRST_POLL: Duration = Duration::from_secs(2);
const LONGEST_POLL: Duration = Duration::from_secs(10);
/// What a wait leaves of the deadline for its last poll and its answer.
const MARGIN: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize, JsonSchema)]
pub struct Waited {
    id: String,
    state: Option<String>,
    /// It reached success or failed.
    done: bool,
    waited_s: u64,
    duration: Option<i64>,
    failed: Vec<String>,
}

fn run_wait(ctx: &Ctx, args: RunIdArgs) -> Result<Waited> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = args.locate(&airflow, ctx)?;
    let started = Instant::now();
    let mut pause = FIRST_POLL;
    loop {
        let left = ctx.deadline().saturating_duration_since(Instant::now());
        let run = match client.get(&id.run_path()) {
            Ok(run) => run,
            // A restart or a dropped connection is not the run failing: ask
            // again, and say so at the deadline.
            Err(error)
                if error.downcast_ref::<Failure>().is_none_or(|failure| {
                    failure.status.is_none() && failure.exit == Exit::Failed
                }) =>
            {
                if left <= MARGIN {
                    return Err(Failure::timed_out(format!(
                        "Airflow did not answer about run {} ({error:#})",
                        id.run_id()
                    ))
                    .hint("agent-cli doctor airflow")
                    .into());
                }
                std::thread::sleep(pause.min(left - MARGIN));
                continue;
            }
            Err(error) => return Err(error),
        };
        let state = text(&run["state"]);
        let mut waited = Waited {
            id: id.run_id(),
            state: state.clone(),
            done: matches!(state.as_deref(), Some("success" | "failed")),
            waited_s: started.elapsed().as_secs(),
            duration: seconds(&run["start_date"], &run["end_date"]),
            failed: Vec::new(),
        };
        match state.as_deref() {
            Some("success") => return Ok(waited),
            Some("failed") => {
                let (tasks, _) = client.list(
                    &format!("{}/taskInstances", id.run_path()),
                    "state=failed",
                    "task_instances",
                    100,
                )?;
                waited.failed = failed_ids(&tasks);
                let hint = match waited.failed.first() {
                    Some(first) => format!("agent-cli airflow task logs {first} --tail 200"),
                    None => format!(
                        "agent-cli airflow run get {} --fields tasks,note",
                        id.run_id()
                    ),
                };
                return Err(
                    Failure::new(Exit::Failed, format!("run {} failed", id.run_id()))
                        .hint(hint)
                        .with_data(waited)
                        .into(),
                );
            }
            _ => {}
        }
        let left = ctx.deadline().saturating_duration_since(Instant::now());
        if left <= MARGIN {
            let paused = state.as_deref() == Some("queued")
                && client.get(&id.dag_path())?["is_paused"].as_bool() == Some(true);
            // With no scheduler, nothing moves the run however long one waits.
            let stalled = !paused
                && client
                    .health()
                    .is_ok_and(|health| health["scheduler"]["status"] != "healthy");
            let hint = if stalled {
                "the scheduler is not running, so the run cannot move: agent-cli doctor airflow"
                    .to_owned()
            } else if paused {
                format!(
                    "the DAG is paused, so the run stays queued: agent-cli airflow dag update {} --paused false",
                    id.dag
                )
            } else {
                format!(
                    "the run keeps going; run the same command again: agent-cli airflow run wait {}",
                    id.run_id()
                )
            };
            return Err(Failure::timed_out(format!(
                "run {} is still {} after {}s",
                id.run_id(),
                state.as_deref().unwrap_or("going"),
                started.elapsed().as_secs()
            ))
            .hint(hint)
            .with_data(waited)
            .into());
        }
        std::thread::sleep(pause.min(left - MARGIN));
        pause = pause.mul_f64(1.5).min(LONGEST_POLL);
    }
}

command! {
    pub RUN_WAIT = ["airflow", "run", "wait"], Read,
    "Wait for a DAG run: exit 0 if it succeeded, 1 if it failed, 124 if still going",
    keywords: ["poll", "finish", "until", "block", "complete", "dag", "dagrun", "watch"],
    example: "airflow run wait etl_nightly/latest",
    timeout: 100,
    run: run_wait,
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RUN, RUN_PATH, airflow, paths, run, tasks, ti};

    #[test]
    fn wait_exits_0_on_success_and_1_with_the_failed_tasks() {
        let (outcome, _) = airflow(
            &["airflow", "run", "wait", &format!("etl_nightly/{RUN}")],
            vec![Answer::json(&run("success"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["done"], true);

        let (outcome, transport) = airflow(
            &["airflow", "run", "wait", &format!("etl_nightly/{RUN}")],
            vec![
                Answer::json(&run("failed")),
                tasks(vec![ti("load_orders", "failed", 2)]),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(
            outcome.json()["failed"],
            json!([format!("etl_nightly/{RUN}/load_orders/2")])
        );
        assert!(
            outcome.stderr.contains(&format!(
                "hint: agent-cli airflow task logs etl_nightly/{RUN}/load_orders/2 --tail 200"
            )),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            paths(&transport)[1],
            format!("{RUN_PATH}/taskInstances?state=failed&limit=100&offset=0")
        );
    }

    #[test]
    fn wait_at_the_deadline_is_124_and_a_paused_dag_gets_the_unpause_hint() {
        let started = Instant::now();
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "run",
                "wait",
                &format!("etl_nightly/{RUN}"),
                "--timeout",
                "3",
            ],
            vec![
                Answer::json(&run("queued")),
                Answer::json(&run("queued")),
                Answer::json(&crate::testing::dag("etl_nightly", true)),
            ],
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert_eq!(outcome.json()["state"], "queued", "its current state");
        assert_eq!(transport.remaining(), 0);
        assert!(
            outcome.stderr.contains("hint: the DAG is paused, so the run stays queued: agent-cli airflow dag update etl_nightly --paused false"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn wait_on_airflow_2_asks_its_health_at_the_deadline() {
        let (outcome, transport) = crate::testing::airflow_v1(
            &[
                "airflow",
                "run",
                "wait",
                &format!("etl_nightly/{RUN}"),
                "--timeout",
                "3",
            ],
            vec![
                Answer::json(&crate::testing::run_v1("running")),
                Answer::json(&crate::testing::run_v1("running")),
                Answer::json(&json!({"scheduler": {"status": "unhealthy"}})),
            ],
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert!(
            outcome.stderr.contains("the scheduler is not running"),
            "{}",
            outcome.stderr
        );
        assert_eq!(paths(&transport).last().unwrap(), "health");
    }
}
