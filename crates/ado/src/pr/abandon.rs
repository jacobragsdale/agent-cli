use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use serde_json::json;

use crate::client::{Ado, Kind};

use super::{PrRow, active_pr, pr_row};

#[derive(clap::Args)]
pub struct AbandonArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
}

fn pr_abandon(ctx: &Ctx, args: AbandonArgs) -> Result<PrRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let (_, repo_id) = active_pr(ctx, &ado, id)?;
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullrequests/{}", id),
        "",
    );
    let done = ado.change(
        ctx,
        Effect::Destructive,
        Method::Patch,
        &url,
        json!({"status": "abandoned"}),
    )?;
    Ok(pr_row(&ado, &done))
}

command! {
    pub PR_ABANDON = ["ado", "pr", "abandon"], Destructive,
    "Abandon (close) a pull request, discarding its changes",
    keywords: ["close", "without", "merging", "discard", "drop", "cancel"],
    example: "ado pr abandon 42 --yes",
    run: pr_abandon,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{dry_run, pr};

    #[test]
    fn abandon_is_one_patch_after_the_status_check() {
        let plans = dry_run(
            &["ado", "pr", "abandon", "17"],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(plans[0]["body"], json!({"status": "abandoned"}));
    }
}
