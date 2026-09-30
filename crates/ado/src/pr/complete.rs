use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use serde_json::json;

use crate::client::{Ado, Kind};

use super::{PrRow, active_pr, pr_row};

#[derive(Clone, Copy, clap::ValueEnum)]
enum Strategy {
    Squash,
    Merge,
    Rebase,
}

#[derive(clap::Args)]
pub struct CompleteArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// How it lands on the target
    #[arg(long, value_enum, default_value = "squash")]
    strategy: Strategy,
    /// Keep the source branch instead of deleting it
    #[arg(long)]
    keep_source: bool,
    /// Leave the linked work items' states alone
    #[arg(long)]
    no_transition: bool,
}

fn pr_complete(ctx: &Ctx, args: CompleteArgs) -> Result<PrRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let (pr, repo_id) = active_pr(ctx, &ado, id)?;
    let strategy = match args.strategy {
        Strategy::Squash => "squash",
        Strategy::Merge => "noFastForward",
        Strategy::Rebase => "rebase",
    };
    // The head it was read at goes with it, so a merge that raced someone
    // else's push is refused by Azure DevOps rather than landing over it.
    let body = json!({
        "status": "completed",
        "lastMergeSourceCommit": pr["lastMergeSourceCommit"],
        "completionOptions": {
            "mergeStrategy": strategy,
            "deleteSourceBranch": !args.keep_source,
            "transitionWorkItems": !args.no_transition,
        },
    });
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullrequests/{}", id),
        "",
    );
    let done = ado.change(ctx, Effect::Destructive, Method::Patch, &url, body)?;
    Ok(pr_row(&ado, &done))
}

command! {
    pub PR_COMPLETE = ["ado", "pr", "complete"], Destructive,
    "Complete (merge) a pull request: squash, merge or rebase",
    keywords: ["merge", "land", "finish", "squash", "ship"],
    example: "ado pr complete 42 --strategy squash --yes",
    run: pr_complete,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{ado, dry_run, pr};

    #[test]
    fn complete_takes_the_head_it_read_and_needs_yes_and_an_active_pr() {
        let plans = dry_run(
            &[
                "ado",
                "pr",
                "complete",
                "17",
                "--strategy",
                "rebase",
                "--keep-source",
            ],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["body"],
            json!({"status": "completed", "lastMergeSourceCommit": {"commitId": "abc123"},
                "completionOptions": {"mergeStrategy": "rebase", "deleteSourceBranch": false, "transitionWorkItems": true}})
        );

        let (outcome, transport) = ado(&["ado", "pr", "complete", "17"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("--yes"), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());

        let mut done = pr(17, false);
        done["status"] = json!("completed");
        let (outcome, _) = ado(
            &["ado", "pr", "complete", "17", "--yes"],
            vec![Answer::json(&done)],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome.stderr.contains("already completed"),
            "{}",
            outcome.stderr
        );

        let mut merged = pr(17, false);
        merged["status"] = json!("completed");
        let (outcome, _) = ado(
            &["ado", "pr", "complete", "17", "--yes"],
            vec![Answer::json(&pr(17, false)), Answer::json(&merged)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["status"], "completed");
    }
}
