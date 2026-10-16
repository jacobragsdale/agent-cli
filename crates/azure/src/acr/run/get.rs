//! `acr run get`: one ACR Tasks run (`registries/{name}/runs/{id}`). A run
//! that failed names its log next.

use agent_cli_core::{Ctx, command};
use anyhow::{Context, Result};

use crate::acr::run::{RUNS_API, RunRow, locate, runs};
use crate::config::Azure;
use crate::net::read;

#[derive(clap::Args)]
pub struct RunGetArgs {
    /// The run: REGISTRY/RUN as acr run list prints it, its ARM id, or RUN with --registry
    run: String,
    /// The registry, for a bare run id when more than one is in reach
    #[arg(long)]
    registry: Option<String>,
}

fn run_get(ctx: &Ctx, args: RunGetArgs) -> Result<RunRow> {
    let azure = Azure::load(ctx)?;
    let (registry, run) = locate(ctx, &azure, &args.run, args.registry.as_deref())?;
    let answer = read(
        ctx,
        &runs(&registry, &format!("/{run}")),
        RUNS_API,
        &format!("acr run list --registry {}", registry.name),
    )?;
    let row = RunRow::new(&registry, &answer)
        .with_context(|| format!("ARM answered run {run} without its id"))?;
    if row.failed() {
        ctx.note(format!("[next: agent-cli acr run logs {}]", row.id));
    }
    Ok(row)
}

command! {
    pub RUN_GET = ["acr", "run", "get"], Read,
    "Show one ACR Tasks run: outcome, trigger, images it pushed, duration, error",
    keywords: ["task", "build", "status", "trigger", "duration", "why", "fail"],
    example: "acr run get contosoacr/ca1f --fields status,images,error",
    run: run_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, next_command};
    use serde_json::json;

    use crate::ACR;
    use crate::testing::{RG, azure, one_registry};

    #[test]
    fn a_failed_run_names_its_log_next_and_a_good_one_names_nothing() {
        let run = |status: &str| {
            Answer::json(
                &json!({"name": "ca1g", "properties": {"runId": "ca1g", "status": status,
                "task": "reports-build", "runErrorMessage": "failed during run, err: exit status 1"}}),
            )
        };
        let id =
            format!("{RG}/providers/Microsoft.ContainerRegistry/registries/contosoacr/runs/ca1g");
        for raw in ["contosoacr/ca1g", "ca1g", id.as_str()] {
            let (outcome, transport) = azure(
                &[ACR],
                &["acr", "run", "get", raw],
                vec![one_registry(), run("Failed")],
            );
            assert_eq!(outcome.code, 0, "{raw}: {outcome:?}");
            assert_eq!(
                transport.sent()[1].url,
                format!("https://management.azure.com{id}?api-version=2019-06-01-preview")
            );
            assert_eq!(
                outcome.json()["error"],
                "failed during run, err: exit status 1"
            );
            assert_eq!(
                next_command(&outcome.stderr),
                Some(vec![
                    "acr".to_owned(),
                    "run".to_owned(),
                    "logs".to_owned(),
                    "contosoacr/ca1g".to_owned()
                ]),
                "{}",
                outcome.stderr
            );
        }
        let (outcome, _) = azure(
            &[ACR],
            &["acr", "run", "get", "contosoacr/ca1g"],
            vec![one_registry(), run("Succeeded")],
        );
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);
        let (outcome, _) = azure(
            &[ACR],
            &[
                "acr",
                "run",
                "get",
                "contosoacr/ca1g",
                "--registry",
                "fabrikamacr",
            ],
            vec![one_registry()],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
