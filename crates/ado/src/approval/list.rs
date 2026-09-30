use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, PREVIEW_API, list, stamp, text};

#[derive(clap::Args)]
pub struct ApprovalListArgs {
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApprovalRow {
    /// What approval approve and reject take.
    id: String,
    pipeline: Option<String>,
    run_id: Option<i64>,
    run: Option<String>,
    instructions: Option<String>,
    created: Option<String>,
    approvers: Vec<Approver>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Approver {
    name: Option<String>,
    status: Option<String>,
}

fn approval_list(ctx: &Ctx, args: ApprovalListArgs) -> Result<Vec<ApprovalRow>> {
    let ado = Ado::load(ctx)?;
    let url = ado.api(
        Some(&ado.code_project),
        "pipelines/approvals",
        &format!("state=pending&$expand=steps&top={}", args.limit + 1),
        PREVIEW_API,
    );
    let answer = ado.get(ctx, &url)?;
    let mut rows: Vec<ApprovalRow> = list(&answer["value"])
        .iter()
        .filter_map(|approval| {
            let owner = &approval["pipeline"]["owner"];
            Some(ApprovalRow {
                id: text(&approval["id"])?,
                pipeline: text(&approval["pipeline"]["name"]),
                run_id: owner["id"].as_i64(),
                run: text(&owner["name"]),
                instructions: text(&approval["instructions"]),
                created: stamp(&approval["createdOn"]),
                approvers: list(&approval["steps"])
                    .iter()
                    .map(|step| Approver {
                        name: text(&step["assignedApprover"]["displayName"]),
                        status: text(&step["status"]),
                    })
                    .collect(),
            })
        })
        .collect();
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!("[first {}; --limit N for more]", args.limit));
    }
    Ok(rows)
}

command! {
    pub APPROVAL_LIST = ["ado", "approval", "list"], Read,
    "List pending pipeline approvals (deployment gates)",
    keywords: ["pending", "waiting", "deploy", "release", "checks", "stage", "blocked"],
    example: "ado approval list --fields id,pipeline,run,approvers",
    run: approval_list,
}
