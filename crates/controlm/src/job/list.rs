use agent_cli_core::{Ctx, When, command};
use anyhow::Result;

use crate::client::{At, ControlM, compact, note_more, with_query};

use super::{JobRow, Status};

#[derive(clap::Args)]
pub struct JobListArgs {
    /// Job name; * matches any characters, a comma separates several
    #[arg(long)]
    name: Option<String>,
    /// Folder name, with * and commas as --name
    #[arg(long)]
    folder: Option<String>,
    /// Control-M/Server (data center) name
    #[arg(long)]
    server: Option<String>,
    /// Application, with * and commas as --name
    #[arg(long)]
    application: Option<String>,
    /// The agent host it runs on
    #[arg(long)]
    host: Option<String>,
    /// Only runs in this status (waiting: any wait-*)
    #[arg(long, value_enum)]
    status: Option<Status>,
    /// Only held jobs
    #[arg(long)]
    held: bool,
    /// Only runs that started at or after this
    #[arg(long)]
    since: Option<When>,
    /// Only runs that started at or before this
    #[arg(long)]
    until: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[command(flatten)]
    at: At,
}

// VERIFY(work): the order rows come back in (none is asked for), and whether
// a search with no filter at all is refused or slow on a busy Enterprise
// Manager.
fn job_list(ctx: &Ctx, args: JobListArgs) -> Result<Vec<JobRow>> {
    let controlm = ControlM::load(ctx.config())?;
    let client = controlm.open(ctx, &args.at)?;
    let offset = client.instance.offset;
    let mut query = vec![
        ("limit", args.limit.to_string()),
        ("jobname", args.name.unwrap_or_default()),
        ("folder", args.folder.unwrap_or_default()),
        ("ctm", args.server.unwrap_or_default()),
        ("application", args.application.unwrap_or_default()),
        ("host", args.host.unwrap_or_default()),
        (
            "status",
            args.status
                .map(Status::words)
                .unwrap_or_default()
                .to_owned(),
        ),
    ];
    if args.held {
        query.push(("held", "true".to_owned()));
    }
    if let Some(since) = args.since {
        query.push(("fromTime", compact(since, offset)));
    }
    if let Some(until) = args.until {
        query.push(("toTime", compact(until, offset)));
    }
    let answer = client.get(&with_query("run/jobs/status", &query))?;
    let rows: Vec<JobRow> = answer["statuses"]
        .as_array()
        .into_iter()
        .flatten()
        .take(args.limit)
        .map(|status| JobRow::from(status, offset))
        .collect();
    let total = answer["total"]
        .as_u64()
        .and_then(|total| usize::try_from(total).ok());
    note_more(ctx, rows.len(), total);
    Ok(rows)
}

command! {
    pub JOB_LIST = ["controlm", "job", "list"], Read,
    "Find failed, running or waiting Control-M jobs by name or time",
    keywords: ["control-m", "ctm", "batch", "find", "search", "failed", "running", "waiting", "held", "status", "monitoring"],
    example: "controlm job list --name 'load_*' --status failed --since 1d --fields id,name,status,start",
    run: job_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{controlm, job, paths};

    #[test]
    fn job_list_searches_runs_with_control_ms_words_and_prints_ids_job_get_takes() {
        let (outcome, transport) = controlm(
            &[
                "controlm",
                "job",
                "list",
                "--name",
                "load_*",
                "--status",
                "failed",
                "--since",
                "2026-09-29T00:00:00Z",
                "--limit",
                "1",
            ],
            vec![Answer::json(&json!({
                "statuses": [job("ctm-prod:00a1b", "load_orders", "Ended Not OK")],
                "returned": 1, "total": 3,
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            paths(&transport),
            [
                "run/jobs/status?limit=1&jobname=load_%2A&status=Ended+Not+OK&fromTime=20260929020000"
            ]
        );
        let got = outcome.json();
        assert_eq!(got[0]["id"], "ctm-prod:00a1b");
        assert_eq!(got[0]["definition"], "ctm-prod/NightlyLoads/load_orders");
        assert_eq!(got[0]["start"], "2026-09-29T00:15:00Z");
        assert_eq!(got[0]["order_date"], "2026-09-29");
        assert!(
            outcome.stderr.contains("[1 of 3; --limit N]"),
            "{}",
            outcome.stderr
        );
    }
}
