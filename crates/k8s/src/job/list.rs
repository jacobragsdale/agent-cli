use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, items, limited, name_has};

use super::{JobRow, row};

#[derive(clap::Args)]
pub struct JobListArgs {
    /// Part of the job name (a CronJob's runs start with its name)
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn job_list(ctx: &Ctx, args: JobListArgs) -> Result<Vec<JobRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "jobs", "-o", "json"])?;
    let rows = items(&listed)
        .filter(|item| name_has(item, args.name.as_deref()))
        .filter_map(|item| row(&target, item))
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub JOB_LIST = ["k8s", "job", "list"], Read,
    "List jobs: status, completions, failures, duration, the cronjob that made each",
    keywords: ["batch", "failed", "succeeded", "completed", "runs", "backoff"],
    example: "k8s job list --cluster prod --namespace web --fields id,status,started,cronjob",
    run: job_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn job_list_shows_status_completions_and_the_cronjob_behind_each() {
        let outcome = k8s(&["k8s", "job", "list", "--cluster", "qa", "--namespace", "qa"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "qa/qa/nightly-report", "name": "nightly-report", "status": "Complete",
                    "completions": "1/1", "failed": 0, "started": "2026-09-10T07:59:00Z", "duration": 90},
                {"id": "qa/qa/cleanup-29819550", "name": "cleanup-29819550", "status": "Complete",
                    "completions": "1/1", "failed": 0, "started": "2026-09-12T00:30:00Z", "duration": 41,
                    "cronjob": "qa/qa/cleanup"},
                {"id": "qa/qa/cleanup-29819580", "name": "cleanup-29819580", "status": "Failed",
                    "completions": "0/1", "failed": 2, "started": "2026-09-12T01:00:00Z", "duration": 300,
                    "cronjob": "qa/qa/cleanup"},
            ])
        );
    }
}
