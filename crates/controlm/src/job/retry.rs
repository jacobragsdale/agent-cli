use agent_cli_core::{Ctx, Effect, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{ControlM, text};

use super::JobIdArgs;

#[derive(Debug, Serialize, JsonSchema)]
pub struct JobRetried {
    id: String,
    /// What Control-M answered: the job's status after the rerun was asked
    /// for, or its message.
    answer: Option<String>,
}

// VERIFY(work): the 9.22 spec answers a rerun with the job's status
// (JobRunStatus); older Enterprise Managers may answer {"message": …}. Both
// are read. Check which statuses Control-M refuses to rerun (Executing, Wait
// …), and with which status and words, so the refusal is exit 5 with a hint.
fn job_retry(ctx: &Ctx, args: JobIdArgs) -> Result<JobRetried> {
    let controlm = ControlM::load(ctx.config())?;
    let (client, id) = args.open(&controlm, ctx)?;
    let answer = client.change(Effect::Write, &format!("run/job/{id}/rerun"), json!({}))?;
    ctx.note(format!("[next: agent-cli controlm job get {id}]"));
    Ok(JobRetried {
        id,
        answer: text(&answer["status"]).or_else(|| text(&answer["message"])),
    })
}

command! {
    pub JOB_RETRY = ["controlm", "job", "retry"], Write,
    "Rerun a Control-M job run, as Rerun does in Control-M",
    keywords: ["control-m", "ctm", "batch", "rerun", "again", "restart", "failed", "retrigger"],
    example: "controlm job retry ctm-prod:00a1b",
    run: job_retry,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Method;
    use agent_cli_core::testing::Answer;

    use crate::testing::{controlm, dry_run, job, paths};

    #[test]
    fn job_retry_reruns_the_job_and_names_job_get_next() {
        let (outcome, transport) = controlm(
            &["controlm", "job", "retry", "ctm-prod:00a1b"],
            vec![Answer::json(&job(
                "ctm-prod:00a1b",
                "load_orders",
                "Wait Condition",
            ))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(paths(&transport), ["run/job/ctm-prod:00a1b/rerun"]);
        assert_eq!(transport.sent()[0].method, Method::Post);
        assert_eq!(outcome.json()["answer"], "Wait Condition");
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli controlm job get ctm-prod:00a1b]")
        );
    }

    #[test]
    fn a_dry_run_plans_the_rerun_and_sends_nothing() {
        let (would, _) = dry_run(&["controlm", "job", "retry", "ctm-prod:00a1b"], vec![]);
        assert_eq!(would[0]["method"], "POST");
        assert!(
            would[0]["url"]
                .as_str()
                .unwrap()
                .ends_with("/run/job/ctm-prod:00a1b/rerun")
        );
        assert_eq!(would[0]["headers"]["x-api-key"], "***");
    }
}
