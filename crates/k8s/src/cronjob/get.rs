use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::job::JobRow;
use crate::kubectl::{At, items, non_empty, owned_by};
use crate::shape::{EventRow, events_in};

use super::{CronJobRow, row};

#[derive(clap::Args)]
pub struct CronjobGetArgs {
    /// The cronjob: its id (cluster/namespace/name), namespace/name, or name
    cronjob: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CronJobDetail {
    #[serde(flatten)]
    row: CronJobRow,
    /// Allow, Forbid (skip a run while one is going) or Replace.
    concurrency_policy: Option<String>,
    /// Seconds a missed run may still start late; past it, it is skipped.
    starting_deadline: Option<i64>,
    /// The runs it still keeps, newest first: what `job get` takes.
    jobs: Vec<JobRow>,
    /// The five newest events about it (SuccessfulCreate, SawCompletedJob,
    /// missed schedules), newest first.
    events: Vec<EventRow>,
}

fn cronjob_get(ctx: &Ctx, args: CronjobGetArgs) -> Result<CronJobDetail> {
    let (target, name) = args.at.named(ctx, &args.cronjob)?;
    let item = target.json(ctx, &["get", "cronjob", &name, "-o", "json"])?;
    let listed = target.json(ctx, &["get", "jobs,events", "-o", "json"])?;
    let mut jobs: Vec<&Value> = items(&listed)
        .filter(|held| held["kind"] == "Job" && owned_by(held, "CronJob", &name))
        .collect();
    // RFC 3339 stamps in UTC sort as text.
    jobs.sort_by_key(|job| std::cmp::Reverse(job["metadata"]["creationTimestamp"].as_str()));
    let jobs: Vec<JobRow> = jobs
        .iter()
        .filter_map(|job| crate::job::row(&target, job))
        .collect();
    if let Some(last) = jobs.iter().find(|job| job.status != "Running")
        && last.status == "Failed"
    {
        ctx.note(format!("[next: agent-cli k8s job get {}]", last.id));
    }
    let row = row(&target, &item)
        .ok_or_else(|| anyhow::anyhow!("kubectl answered a cronjob with no name"))?;
    Ok(CronJobDetail {
        row,
        concurrency_policy: non_empty(&item["spec"]["concurrencyPolicy"]).map(str::to_owned),
        starting_deadline: item["spec"]["startingDeadlineSeconds"].as_i64(),
        jobs,
        events: events_in(&target, &listed, &item),
    })
}

command! {
    pub CRONJOB_GET = ["k8s", "cronjob", "get"], Read,
    "Describe a cronjob: recent runs with status, why one did not run",
    keywords: ["runs", "history", "failed", "fail", "last", "missed", "not", "running"],
    example: "k8s cronjob get prod/web/orders-export --fields schedule,last_success,jobs",
    run: cronjob_get,
}

#[cfg(test)]
mod tests {
    use crate::testing::k8s;

    #[test]
    fn a_cronjob_whose_last_run_failed_names_that_run() {
        let outcome = k8s(&["k8s", "cronjob", "get", "qa/qa/cleanup"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["concurrency_policy"], "Forbid");
        assert_eq!(got["jobs"][0]["id"], "qa/qa/cleanup-29819580");
        assert_eq!(got["jobs"][0]["status"], "Failed");
        assert_eq!(got["jobs"][1]["status"], "Complete");
        assert_eq!(got["events"][0]["reason"], "SawCompletedJob");
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli k8s job get qa/qa/cleanup-29819580]"),
            "{}",
            outcome.stderr
        );
    }
}
