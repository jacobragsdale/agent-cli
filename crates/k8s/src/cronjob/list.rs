use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, items, limited, name_has};

use super::{CronJobRow, row};

#[derive(clap::Args)]
pub struct CronjobListArgs {
    /// Part of the cronjob name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn cronjob_list(ctx: &Ctx, args: CronjobListArgs) -> Result<Vec<CronJobRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "cronjobs", "-o", "json"])?;
    let rows = items(&listed)
        .filter(|item| name_has(item, args.name.as_deref()))
        .filter_map(|item| row(&target, item))
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub CRONJOB_LIST = ["k8s", "cronjob", "list"], Read,
    "List cronjobs: schedule, time zone, suspended, last run and last success",
    keywords: ["cron", "schedules", "suspend"],
    example: "k8s cronjob list --cluster prod --namespace web --fields id,schedule,last_schedule,last_success",
    run: cronjob_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn cronjob_list_shows_the_schedule_and_the_last_run_and_success() {
        let outcome = k8s(&[
            "k8s",
            "cronjob",
            "list",
            "--cluster",
            "qa",
            "--namespace",
            "qa",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "qa/qa/cleanup", "name": "cleanup", "schedule": "*/30 * * * *",
                "time_zone": "Etc/UTC", "suspended": false, "last_schedule": "2026-09-12T01:00:00Z",
                "last_success": "2026-09-12T00:30:41Z"}])
        );
    }
}
