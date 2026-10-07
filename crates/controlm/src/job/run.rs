use agent_cli_core::{Ctx, Effect, command};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{At, ControlM, text};
use crate::definition::DefRef;

use super::JobRow;

#[derive(clap::Args)]
pub struct JobRunArgs {
    /// The definition: SERVER/FOLDER/JOB, as definition list prints it; JOB
    /// may be * for every job in the folder
    definition: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct JobOrdered {
    /// Control-M's id for this order.
    run_id: String,
    /// The runs it ordered, with the ids job get takes.
    jobs: Vec<JobRow>,
}

/// Orders the job into today's plan whatever its calendar says (what the
/// web client calls Order with "ignore scheduling criteria"), then reads
/// back the runs it made.
// VERIFY(work), in order:
// - the order body: ignoreCriteria true runs a job its calendar does not
//   schedule today; createDuplicate is left to Control-M's default;
// - that `jobs` takes a job name, and * for the whole folder;
// - how a folder inside a folder is named in `folder` (A/B?);
// - that run/status/RUN_ID lists the ordered jobs right away; if it is empty
//   at first, ask again within the deadline before answering.
fn job_run(ctx: &Ctx, args: JobRunArgs) -> Result<JobOrdered> {
    let controlm = ControlM::load(ctx.config())?;
    let client = controlm.open(ctx, &args.at)?;
    let definition = DefRef::parse(&args.definition)?;
    let body = json!({
        "ctm": definition.server,
        "folder": definition.folder,
        "jobs": definition.job,
        "ignoreCriteria": true,
    });
    let answer = client.change(Effect::Write, "run/order", body)?;
    let run_id = text(&answer["runId"]).context("Control-M answered the order without a runId")?;
    let statuses = client.get(&format!("run/status/{run_id}"))?;
    let offset = client.instance.offset;
    let jobs: Vec<JobRow> = statuses["statuses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|status| JobRow::from(status, offset))
        .collect();
    if let [job] = jobs.as_slice() {
        ctx.note(format!("[next: agent-cli controlm job get {}]", job.id));
    }
    Ok(JobOrdered { run_id, jobs })
}

command! {
    pub JOB_RUN = ["controlm", "job", "run"], Write,
    "Run a Control-M job, or a whole folder, now and outside its schedule",
    keywords: ["control-m", "ctm", "batch", "trigger", "force", "start", "kick", "manual", "now", "submit"],
    example: "controlm job run ctm-prod/NightlyLoads/load_orders",
    run: job_run,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{controlm, dry_run, job, paths};

    #[test]
    fn job_run_orders_the_definition_and_prints_the_run_it_made() {
        let (outcome, transport) = controlm(
            &[
                "controlm",
                "job",
                "run",
                "ctm-prod/NightlyLoads/load_orders",
            ],
            vec![
                Answer::json(
                    &json!({"runId": "7cba67de-7e23-48d3-9d5a-2d4a3c7d5e1f", "statusURI": "x"}),
                ),
                Answer::json(
                    &json!({"statuses": [job("ctm-prod:00a1d", "load_orders", "Wait Condition")]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            paths(&transport),
            [
                "run/order",
                "run/status/7cba67de-7e23-48d3-9d5a-2d4a3c7d5e1f"
            ]
        );
        assert_eq!(
            transport.sent()[0].body,
            Some(
                json!({"ctm": "ctm-prod", "folder": "NightlyLoads", "jobs": "load_orders", "ignoreCriteria": true})
            )
        );
        assert_eq!(outcome.json()["jobs"][0]["id"], "ctm-prod:00a1d");
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli controlm job get ctm-prod:00a1d]")
        );
    }

    #[test]
    fn a_dry_run_plans_the_order_and_sends_nothing() {
        let (would, _) = dry_run(
            &["controlm", "job", "run", "ctm-prod/NightlyLoads/*"],
            vec![],
        );
        assert_eq!(would[0]["body"]["jobs"], "*");
    }

    #[test]
    fn a_read_only_instance_refuses_before_anything_is_sent() {
        let config = format!("{}read_only = true\n", crate::testing::CONFIG);
        let (outcome, transport) = crate::testing::controlm_with(
            &config,
            &[
                "controlm",
                "job",
                "run",
                "ctm-prod/NightlyLoads/load_orders",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }
}
