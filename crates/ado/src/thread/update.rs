use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Ado, text};

use super::{Status, locate, wire};

#[derive(clap::Args)]
pub struct ThreadUpdateArgs {
    /// The thread: PR/THREAD (436/7) as thread list and pr get print it, or its web URL
    id: String,
    /// fixed resolves it, active reopens it
    #[arg(long, value_enum)]
    status: Status,
    /// The pull request, when the id is the thread's number alone
    #[arg(long)]
    pr: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ThreadUpdated {
    id: String,
    status: Option<String>,
}

fn thread_update(ctx: &Ctx, args: ThreadUpdateArgs) -> Result<ThreadUpdated> {
    let ado = Ado::load(ctx)?;
    let (id, path) = locate(ctx, &ado, &args.id, args.pr.as_deref())?;
    let url = ado.code(&path, "");
    let thread = ado.change(
        ctx,
        Effect::Write,
        Method::Patch,
        &url,
        json!({"status": wire(args.status)}),
    )?;
    Ok(ThreadUpdated {
        id,
        status: text(&thread["status"]),
    })
}

command! {
    pub THREAD_UPDATE = ["ado", "thread", "update"], Write,
    "Resolve, reopen or close a pull request review thread",
    keywords: ["resolve", "reopen", "fixed", "wontfix", "close", "status", "mark"],
    example: "ado thread update 436/7 --status fixed",
    run: thread_update,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, dry_run, pr};

    #[test]
    fn a_status_patches_the_thread_named_by_its_url() {
        let url = "https://dev.azure.com/contoso/Fabrikam/_git/web/pullrequest/17?discussionId=7";
        let plans = dry_run(
            &["ado", "thread", "update", url, "--status", "active"],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/git/repositories/r-1/pullRequests/17/threads/7?api-version=7.1")
        );
        assert_eq!(plans[0]["body"], json!({"status": "active"}));

        let (outcome, _) = ado(
            &["ado", "thread", "update", "17/7", "--status", "wontFix"],
            vec![
                Answer::json(&pr(17, false)),
                Answer::json(&json!({"id": 7, "status": "wontFix"})),
            ],
        );
        assert_eq!(outcome.json(), json!({"id": "17/7", "status": "wontFix"}));
        let (outcome, _) = ado(
            &["ado", "thread", "update", "17/7", "--status", "17/8"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
