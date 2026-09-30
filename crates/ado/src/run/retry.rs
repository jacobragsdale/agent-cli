use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use serde_json::json;

use crate::client::{Ado, Kind};

use super::{RunIdArgs, RunRow, run_row};

fn run_retry(ctx: &Ctx, args: RunIdArgs) -> Result<RunRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.id)?;
    let url = ado.code(&format!("build/builds/{}", id), "retry=true");
    let run = ado.change(ctx, Effect::Write, Method::Patch, &url, json!({}))?;
    Ok(run_row(&run))
}

command! {
    pub RUN_RETRY = ["ado", "run", "retry"], Write,
    "Retry the failed jobs of a finished run",
    keywords: ["rerun", "again", "restart", "failed", "build", "flaky"],
    example: "ado run retry 1234",
    run: run_retry,
}
