use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::ControlM;

use super::JobIdArgs;

#[derive(clap::Args)]
pub struct JobLogsArgs {
    #[command(flatten)]
    id: JobIdArgs,
    /// How many of the last lines (0 for all)
    #[arg(long, default_value_t = 200)]
    tail: usize,
    /// Which execution's output (1 is the first); the latest by default
    #[arg(long, conflicts_with = "events")]
    execution: Option<u32>,
    /// Control-M's own log of the job (ordered, submitted, ended, rerun)
    /// instead of what the job printed
    #[arg(long)]
    events: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct JobLogs {
    id: String,
    /// output: what the job printed (its sysout); events: Control-M's log of it.
    source: &'static str,
    /// The execution read, when --execution named one.
    execution: Option<u32>,
    lines: usize,
    /// The last lines kept (--tail).
    kept: usize,
    text: String,
}

// VERIFY(work): what output answers for a job still executing, and for one
// whose output the agent no longer keeps (the status code and message), so
// both say what to do next.
fn job_logs(ctx: &Ctx, args: JobLogsArgs) -> Result<JobLogs> {
    let controlm = ControlM::load(ctx.config())?;
    let (client, id) = args.id.open(&controlm, ctx)?;
    let (source, path) = match (args.events, args.execution) {
        (true, _) => ("events", format!("run/job/{id}/log")),
        (false, Some(execution)) => ("output", format!("run/job/{id}/output?runNo={execution}")),
        (false, None) => ("output", format!("run/job/{id}/output")),
    };
    let text = client.text(&path)?;
    let lines: Vec<&str> = text.lines().collect();
    let keep = if args.tail == 0 {
        lines.len()
    } else {
        args.tail.min(lines.len())
    };
    Ok(JobLogs {
        id,
        source,
        execution: args.execution,
        lines: lines.len(),
        kept: keep,
        text: lines[lines.len() - keep..].join("\n"),
    })
}

command! {
    pub JOB_LOGS = ["controlm", "job", "logs"], Read,
    "Read the tail of what a Control-M job run printed, or Control-M's log of it",
    keywords: ["control-m", "ctm", "batch", "output", "sysout", "log", "error", "why", "failed", "events", "show"],
    example: "controlm job logs ctm-prod:00a1b --tail 50",
    run: job_logs,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;

    use crate::testing::{controlm, paths};

    #[test]
    fn job_logs_tails_the_output_of_the_execution_asked_for() {
        let (outcome, transport) = controlm(
            &[
                "controlm",
                "job",
                "logs",
                "ctm-prod:00a1b",
                "--execution",
                "2",
                "--tail",
                "2",
            ],
            vec![Answer::ok(
                "loading orders for 2026-09-28\nread 1204 rows\nERROR: order 88123 has no customer_id\nexit code 1",
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(paths(&transport), ["run/job/ctm-prod:00a1b/output?runNo=2"]);
        let sent = &transport.sent()[0];
        assert!(
            sent.headers
                .contains(&("Accept".into(), "text/plain".into()))
        );
        let got = outcome.json();
        assert_eq!(
            (got["lines"].as_u64(), got["kept"].as_u64()),
            (Some(4), Some(2))
        );
        assert_eq!(
            got["text"],
            "ERROR: order 88123 has no customer_id\nexit code 1"
        );
        assert_eq!(got["source"], "output");
    }

    #[test]
    fn events_reads_control_ms_log_of_the_job() {
        let (outcome, transport) = controlm(
            &["controlm", "job", "logs", "ctm-prod:00a1b", "--events"],
            vec![Answer::ok(
                "SEVERITY 0 ORDERED\nSEVERITY 0 SUBMITTED\nENDED NOTOK",
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(paths(&transport), ["run/job/ctm-prod:00a1b/log"]);
        assert_eq!(outcome.json()["source"], "events");
    }
}
