use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{ControlM, stamp, text};

use super::{JobIdArgs, JobRow};

#[derive(Debug, Serialize, JsonSchema)]
pub struct JobDetail {
    #[serde(flatten)]
    job: JobRow,
    description: Option<String>,
    cyclic: bool,
    /// When Control-M expects it to start and end, from its earlier runs.
    estimated_start: Option<String>,
    estimated_end: Option<String>,
    /// Why it has not started, in Control-M's words: the conditions,
    /// resources or host it waits for. Empty unless it waits.
    waiting: Vec<String>,
}

fn job_get(ctx: &Ctx, args: JobIdArgs) -> Result<JobDetail> {
    let controlm = ControlM::load(ctx.config())?;
    let (client, id) = args.open(&controlm, ctx)?;
    let status = client.get(&format!("run/job/{id}/status"))?;
    let offset = client.instance.offset;
    let job = JobRow::from(&status, offset);
    // VERIFY(work): that a waiting job's status starts with "Wait", and what
    // waitingInfo answers (the spec: a list of strings).
    let waiting = if job.status.as_deref().is_some_and(|s| s.starts_with("Wait")) {
        client
            .get(&format!("run/job/{id}/waitingInfo"))?
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(text)
            .collect()
    } else {
        Vec::new()
    };
    if job.failed() {
        ctx.note(format!("[next: agent-cli controlm job logs {id}]"));
    }
    Ok(JobDetail {
        description: text(&status["description"]),
        cyclic: status["cyclic"].as_bool().unwrap_or(false),
        estimated_start: stamp(&status["estimatedStartTime"][0], offset),
        estimated_end: stamp(&status["estimatedEndTime"][0], offset),
        waiting,
        job,
    })
}

command! {
    pub JOB_GET = ["controlm", "job", "get"], Read,
    "Show a Control-M job's status, and why it waits",
    keywords: ["control-m", "ctm", "batch", "status", "state", "why", "waiting", "stuck", "condition", "resource", "held", "failed", "show"],
    example: "controlm job get ctm-prod:00a1b",
    run: job_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, next_command};
    use serde_json::json;

    use crate::testing::{controlm, job, paths};

    #[test]
    fn a_waiting_job_says_what_it_waits_for() {
        let (outcome, transport) = controlm(
            &["controlm", "job", "get", "ctm-prod:00a1c"],
            vec![
                Answer::json(&job("ctm-prod:00a1c", "publish_orders", "Wait Condition")),
                Answer::json(&json!([
                    "Wait for condition load_orders-OK-TO-publish_orders ODAT"
                ])),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            paths(&transport),
            [
                "run/job/ctm-prod:00a1c/status",
                "run/job/ctm-prod:00a1c/waitingInfo"
            ]
        );
        let got = outcome.json();
        assert_eq!(got["status"], "Wait Condition");
        assert_eq!(
            got["waiting"][0],
            "Wait for condition load_orders-OK-TO-publish_orders ODAT"
        );
    }

    #[test]
    fn a_failed_job_names_its_logs_next() {
        let (outcome, transport) = controlm(
            &["controlm", "job", "get", "ctm-prod:00a1b"],
            vec![Answer::json(&job(
                "ctm-prod:00a1b",
                "load_orders",
                "Ended Not OK",
            ))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent().len(),
            1,
            "no waitingInfo for a job that ran"
        );
        assert_eq!(
            next_command(&outcome.stderr),
            Some(vec![
                "controlm".into(),
                "job".into(),
                "logs".into(),
                "ctm-prod:00a1b".into()
            ])
        );
    }

    #[test]
    fn words_for_an_id_are_exit_2_naming_the_search() {
        let (outcome, transport) = controlm(&["controlm", "job", "get", "load_orders"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli controlm job list --name load_orders")
        );
        assert!(transport.sent().is_empty());
    }
}
