use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use serde_json::json;

use crate::client::{Ado, Kind};

use super::{RunIdArgs, RunRow, build_url, run_row};

fn run_cancel(ctx: &Ctx, args: RunIdArgs) -> Result<RunRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.id)?;
    let body = json!({"status": "cancelling"});
    let run = ado.change(
        ctx,
        Effect::Destructive,
        Method::Patch,
        &build_url(&ado, id),
        body,
    )?;
    let run = run_row(&run);
    // Azure DevOps answers a finished run as it is, with a 200.
    if run.status.as_deref() == Some("completed") {
        return Err(Failure::conflict(format!(
            "run {id} had already finished {}; nothing was canceled",
            run.result.as_deref().unwrap_or("without a result")
        ))
        .hint(format!("agent-cli ado run get {id}"))
        .with_data(run)
        .into());
    }
    Ok(run)
}

command! {
    pub RUN_CANCEL = ["ado", "run", "cancel"], Destructive,
    "Cancel a run that is queued or in progress",
    keywords: ["stop", "abort", "kill", "halt", "build"],
    example: "ado run cancel 1234 --yes",
    run: run_cancel,
}
