//! `aisearch indexer wait`: polls `/status` until the last run started at or
//! after `--since` and is no longer in progress. The service offers nothing
//! else to wait on.

use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Failure, When, command};
use anyhow::Result;

use crate::aisearch::holder;
use crate::aisearch::indexer::{
    IndexerDetail, definitions, detail, failed, named, next_note, status,
};
use crate::client::{parse_stamp, text};
use crate::config::Azure;

const POLL: Duration = Duration::from_secs(5);
/// What a wait leaves of the deadline for its last poll and its answer.
const MARGIN: Duration = Duration::from_secs(2);

#[derive(clap::Args)]
pub struct IndexerWaitArgs {
    /// The indexer: SERVICE/INDEXER from indexer list, or a bare name
    indexer: String,
    /// The service that holds it; needed when more than one does
    #[arg(long)]
    service: Option<String>,
    /// Wait for a run that started at or after this (indexer run prints it); default: the current one
    #[arg(long)]
    since: Option<When>,
}

fn indexer_wait(ctx: &Ctx, args: IndexerWaitArgs) -> Result<IndexerDetail> {
    let azure = Azure::load(ctx)?;
    let (service, name) = named(&args.indexer, args.service.as_deref())?;
    let search = holder(ctx, &azure, "indexers", &name, service.as_deref())?;
    let (indexer, source) = definitions(&search, &name)?;
    let started = Instant::now();
    loop {
        let status = status(&search, &name)?;
        let last = &status["lastResult"];
        let run = text(&last["status"]);
        let new_enough = match (&args.since, text(&last["startTime"])) {
            (None, _) => true,
            (Some(since), Some(start)) => parse_stamp(&start).is_some_and(|start| start >= since.0),
            (Some(_), None) => false,
        };
        let row = detail(ctx, &search, &indexer, &source, &status);
        if new_enough && run.is_some() && run.as_deref() != Some("inProgress") {
            if run.as_deref() == Some("success") {
                next_note(ctx, &row);
                return Ok(row);
            }
            return Err(failed(&row).with_data(&row).into());
        }
        let left = ctx.deadline().saturating_duration_since(Instant::now());
        if left <= MARGIN {
            let state = if new_enough {
                run.unwrap_or_else(|| "not started".to_owned())
            } else {
                "not started yet".to_owned()
            };
            return Err(Failure::timed_out(format!(
                "{}'s run is {state} after {}s",
                row.id,
                started.elapsed().as_secs()
            ))
            .hint(format!(
                "it keeps going; run the same command again: agent-cli aisearch indexer wait {}",
                row.id
            ))
            .with_data(&row)
            .into());
        }
        std::thread::sleep(POLL.min(left - MARGIN));
    }
}

command! {
    pub INDEXER_WAIT = ["aisearch", "indexer", "wait"], Read,
    "Wait for an AI Search indexer run to end: exit 0 succeeded, 1 failed, 124 going",
    keywords: ["poll", "finish", "until", "block", "complete", "done", "watch"],
    example: "aisearch indexer wait srch-contoso-prod/orders-sql --since 2026-09-29T00:39:00Z",
    timeout: 100,
    run: indexer_wait,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, azure};

    fn answers(statuses: Vec<serde_json::Value>) -> Vec<Answer> {
        let mut answers = vec![
            testing::inventory(vec![testing::search_service(
                "srch-contoso-prod",
                "aadOrApiKey",
            )]),
            Answer::json(
                &json!({"name": "orders-sql", "dataSourceName": "orders-sql", "targetIndexName": "orders"}),
            ),
            Answer::json(
                &json!({"name": "orders-sql", "type": "azuresql", "container": {"name": "dbo.orders"}}),
            ),
        ];
        answers.extend(statuses.iter().map(Answer::json));
        answers
    }

    fn status(run: &str, start: &str) -> serde_json::Value {
        json!({"status": "running", "lastResult": {"status": run, "startTime": start, "itemsProcessed": 3, "itemsFailed": 0}})
    }

    #[test]
    fn wait_skips_an_older_run_and_exits_by_how_the_new_one_ended() {
        let argv = [
            "aisearch",
            "indexer",
            "wait",
            "srch-contoso-prod/orders-sql",
            "--since",
            "2026-09-29T12:00:00Z",
        ];
        let started = std::time::Instant::now();
        let (outcome, transport) = azure(
            &[AISEARCH],
            &argv,
            answers(vec![
                status("success", "2026-09-29T11:00:00.000Z"),
                status("success", "2026-09-29T12:00:01.500Z"),
            ]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["last"]["start"], "2026-09-29T12:00:01Z");
        assert_eq!(transport.remaining(), 0, "polled twice");
        assert!(started.elapsed() >= std::time::Duration::from_secs(4));

        let (outcome, _) = azure(
            &[AISEARCH],
            &argv,
            answers(vec![status("transientFailure", "2026-09-29T12:00:01Z")]),
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(outcome.json()["last"]["status"], "transientFailure");
    }

    #[test]
    fn wait_at_the_deadline_is_124_with_the_last_status() {
        let (outcome, _) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "indexer",
                "wait",
                "srch-contoso-prod/orders-sql",
                "--timeout",
                "2",
            ],
            answers(vec![status("inProgress", "2026-09-29T12:00:01Z")]),
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert_eq!(outcome.json()["last"]["status"], "inProgress");
    }
}
