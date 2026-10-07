use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::{At, ControlM, note_more, with_query};

use super::{DefRow, found};

#[derive(clap::Args)]
pub struct DefinitionListArgs {
    /// Job name; * matches any characters
    #[arg(long)]
    name: Option<String>,
    /// Folder name, with * as --name
    #[arg(long)]
    folder: Option<String>,
    /// Control-M/Server (data center) name
    #[arg(long)]
    server: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[command(flatten)]
    at: At,
}

// VERIFY(work): deploy/jobs reads the whole folder of every match before
// the limit applies; time `definition list` with no filter on a big
// Enterprise Manager, and require --name or --folder if it is slow.
fn definition_list(ctx: &Ctx, args: DefinitionListArgs) -> Result<Vec<DefRow>> {
    let controlm = ControlM::load(ctx.config())?;
    let client = controlm.open(ctx, &args.at)?;
    let server = args.server.unwrap_or_else(|| "*".to_owned());
    let query = [
        ("format", "json".to_owned()),
        ("ctm", server.clone()),
        ("folder", args.folder.unwrap_or_else(|| "*".to_owned())),
        ("job", args.name.unwrap_or_else(|| "*".to_owned())),
    ];
    let answer = client.get(&with_query("deploy/jobs", &query))?;
    let mut rows: Vec<DefRow> = found(&answer, &server)
        .into_iter()
        .map(|found| found.row)
        .collect();
    let total = rows.len();
    rows.truncate(args.limit);
    note_more(ctx, rows.len(), Some(total));
    Ok(rows)
}

command! {
    pub DEFINITION_LIST = ["controlm", "definition", "list"], Read,
    "Find which Control-M folder defines a job, and what it runs, where and as whom",
    keywords: ["control-m", "ctm", "batch", "find", "search", "folder", "planning", "command", "script", "host", "which"],
    example: "controlm definition list --name 'load_*' --fields id,command,host",
    run: definition_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{controlm, paths};

    #[test]
    fn definition_list_finds_jobs_in_folders_with_ids_job_run_takes() {
        let (outcome, transport) = controlm(
            &["controlm", "definition", "list", "--name", "load_*"],
            vec![Answer::json(&json!({"NightlyLoads": {
                "Type": "Folder", "ControlmServer": "ctm-prod",
                "load_orders": {"Type": "Job:Command", "Command": "/opt/etl/load_orders.sh", "RunAs": "etl"}
            }}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            paths(&transport),
            ["deploy/jobs?format=json&ctm=%2A&folder=%2A&job=load_%2A"]
        );
        let got = outcome.json();
        assert_eq!(got[0]["id"], "ctm-prod/NightlyLoads/load_orders");
        assert_eq!(got[0]["run_as"], "etl");
    }
}
