use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use serde_json::json;

use crate::client::{Ado, Kind};

use super::{RunIdArgs, RunRow, build_url, new_run, run_row};

fn run_retry(ctx: &Ctx, args: RunIdArgs) -> Result<RunRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.id)?;
    // Azure DevOps retries only a run that finished failed, and refuses the
    // rest with a 500; the run's state says which beforehand.
    let run = run_row(&ado.get(ctx, &build_url(&ado, id))?);
    match (run.status.as_deref(), run.result.as_deref()) {
        (Some("completed"), Some("failed")) => {}
        (Some("completed"), result) => {
            return Err(Failure::conflict(format!(
                "run {id} finished {}, and retry reruns the jobs of a failed run",
                result.unwrap_or("without a result")
            ))
            .hint(format!("{}  (a new run)", new_run(&run)))
            .into());
        }
        (status, _) => {
            return Err(Failure::conflict(format!(
                "run {id} is still {}, and retry reruns the jobs of a finished run",
                status.unwrap_or("going")
            ))
            .hint(format!("agent-cli ado run wait {id}"))
            .into());
        }
    }
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
