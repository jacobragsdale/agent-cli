//! `acr run logs`: a run's log, the way `az acr task logs` reads a finished
//! one. ARM's `listLogSasUrl` (a POST that only reads) hands back a SAS URL
//! for the log blob, and that is fetched with no token: the SAS is the
//! credential. It is held as a `Secret` from the moment it arrives, so it is
//! never printed, noted or left in an error.

use agent_cli_core::{Ctx, Method, Request, Secret, command, host_under};
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Serialize;

use crate::acr::run::{RUNS_API, locate, runs};
use crate::client::{ARM, bearer, text};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct RunLogsArgs {
    /// The run: REGISTRY/RUN as acr run list prints it, its ARM id, or RUN with --registry
    run: String,
    /// The registry, for a bare run id when more than one is in reach
    #[arg(long)]
    registry: Option<String>,
    /// How many of the last lines
    #[arg(long, default_value_t = 200)]
    tail: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunLog {
    /// `registry/runid`.
    id: String,
    /// How many lines the log has in all.
    lines: usize,
    /// The last `--tail` of them.
    text: String,
}

fn run_logs(ctx: &Ctx, args: RunLogsArgs) -> Result<RunLog> {
    let azure = Azure::load(ctx)?;
    let (registry, run) = locate(ctx, &azure, &args.run, args.registry.as_deref())?;
    let url = format!(
        "https://management.azure.com{}?api-version={RUNS_API}",
        runs(&registry, &format!("/{run}/listLogSasUrl"))
    );
    let answer = ctx
        .read(Request::new(Method::Query, url).auth(&bearer(ctx, ARM)))?
        .json()?;
    let link = Secret::new(
        text(&answer["logLink"]).with_context(|| format!("ARM gave run {run} no log link"))?,
    );
    if !host_under(link.expose(), ".blob.core.windows.net") {
        bail!("the log link for run {run} is not a blob storage address; it was not fetched");
    }
    // VERIFY(work): that the blob is plain UTF-8 text, not gzipped, whatever
    // content type it is served with.
    let log = ctx.read(Request::get(link.expose()))?;
    let log = String::from_utf8_lossy(&log.into_bytes()).into_owned();
    let lines: Vec<&str> = log.lines().collect();
    let kept = &lines[lines.len().saturating_sub(args.tail)..];
    Ok(RunLog {
        id: format!("{}/{run}", registry.name),
        lines: lines.len(),
        text: kept.join("\n"),
    })
}

command! {
    pub RUN_LOGS = ["acr", "run", "logs"], Read,
    "Read the tail of an ACR Tasks run's log, each step's output",
    keywords: ["task", "output", "steps", "docker"],
    example: "acr run logs contosoacr/ca1g --tail 50",
    run: run_logs,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::ACR;
    use crate::testing::{RG, azure, one_registry};

    const SIG: &str = "fixture-sas-signature-0f9e";

    fn link(host: &str) -> Answer {
        Answer::json(&json!({"logLink": format!(
            "https://{host}/logs/ca1g/rawtext.log?sv=2021-08-06&sr=b&sig={SIG}&se=2026-09-29T13%3A00%3A00Z"
        )}))
    }

    #[test]
    fn the_log_is_read_with_no_token_and_the_sas_is_never_printed() {
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "run", "logs", "contosoacr/ca1g", "--tail", "2"],
            vec![
                one_registry(),
                link("contosoacrlogs.blob.core.windows.net"),
                Answer::status(
                    200,
                    "Step 1/4 : FROM base\nStep 2/4 : RUN make\nmake: *** [all] Error 1\nRun ID: ca1g failed after 41s.\n",
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "contosoacr/ca1g", "lines": 4,
                "text": "make: *** [all] Error 1\nRun ID: ca1g failed after 41s."})
        );
        let sent = transport.sent();
        assert_eq!(
            sent[1].url,
            format!(
                "https://management.azure.com{RG}/providers/Microsoft.ContainerRegistry/registries/contosoacr/runs/ca1g/listLogSasUrl?api-version=2019-06-01-preview"
            )
        );
        assert!(sent[2].url.contains(SIG));
        assert_eq!(sent[2].authorization, None, "the SAS is the credential");
        assert!(!outcome.stdout.contains(SIG) && !outcome.stderr.contains(SIG));
    }

    #[test]
    fn a_refused_blob_or_a_link_off_blob_storage_leaves_the_sas_out_of_the_error() {
        let (outcome, _) = azure(
            &[ACR],
            &["acr", "run", "logs", "contosoacr/ca1g"],
            vec![
                one_registry(),
                link("contosoacrlogs.blob.core.windows.net"),
                Answer::status(403, "<Error><Code>AuthenticationFailed</Code></Error>"),
            ],
        );
        assert_ne!(outcome.code, 0, "{outcome:?}");
        assert!(!outcome.stderr.contains(SIG), "{}", outcome.stderr);

        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "run", "logs", "contosoacr/ca1g"],
            vec![one_registry(), link("evil.example")],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("not a blob storage address"),
            "{}",
            outcome.stderr
        );
        assert!(!outcome.stderr.contains(SIG), "{}", outcome.stderr);
        assert_eq!(transport.sent().len(), 2, "the link was not fetched");
    }
}
