use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::{Dd, limited};

use super::{DowntimeRow, downtime_row};

#[derive(clap::Args)]
pub struct DowntimeListArgs {
    /// Only downtimes that name this monitor id
    #[arg(long)]
    monitor: Option<i64>,
    /// Include ended and canceled downtimes
    #[arg(long)]
    all: bool,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn downtime_list(ctx: &Ctx, args: DowntimeListArgs) -> Result<Vec<DowntimeRow>> {
    let dd = Dd::load(ctx)?;
    // A filter on the monitor is applied here, so it reads a whole page.
    let page = if args.monitor.is_some() {
        1000
    } else {
        args.limit.min(1000)
    };
    let found = dd.get(
        ctx,
        "/api/v2/downtime",
        &[
            ("current_only", (!args.all).to_string()),
            ("page[limit]", page.to_string()),
        ],
    )?;
    let rows = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(downtime_row)
        .filter(|row| {
            args.monitor
                .is_none_or(|monitor| row.monitor == Some(monitor))
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub DOWNTIME_LIST = ["dd", "downtime", "list"], Read,
    "List Datadog downtimes: which monitors are muted, for what scope and until when",
    keywords: ["muted", "silenced", "maintenance", "scheduled", "snoozed", "active", "current"],
    example: "dd downtime list --monitor 4711 --fields id,status,end",
    run: downtime_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{dd, downtime};

    #[test]
    fn downtime_list_filters_by_monitor_and_prints_utc() {
        let (outcome, transport) = dd(
            &["dd", "downtime", "list", "--monitor", "4711"],
            vec![Answer::json(&json!({
                "data": [
                    downtime("00000000-0000-4000-8000-00000000d001", 4711, "2026-09-29T12:10:00+00:00"),
                    downtime("00000000-0000-4000-8000-00000000d002", 4712, "2026-09-29T13:00:00+00:00")
                ],
                "meta": {"page": {"total_filtered_count": 2}}
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], "00000000-0000-4000-8000-00000000d001");
        assert_eq!(rows[0]["start"], "2026-09-29T11:40:00Z");
        assert_eq!(rows[0]["end"], "2026-09-29T12:10:00Z");
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v2/downtime?current_only=true&page%5Blimit%5D=1000"
        );
    }
}
