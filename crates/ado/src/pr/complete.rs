use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use serde_json::json;

use crate::client::{Ado, Kind, short_branch, text};

use super::{PrRow, active_pr, fetch_pr, pr_row};

/// How long a queued merge gets to land or fail before the answer.
const SETTLE: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_secs(2);

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
    if pr["isDraft"].as_bool() == Some(true) {
        return Err(Failure::conflict(format!(
            "pull request {id} is a draft, and a draft cannot be completed"
        ))
        .hint(format!(
            "agent-cli ado pr update {id} --draft false, then run this again"
        ))
        .into());
    }
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
    let mut done = ado
        .change(ctx, Effect::Destructive, Method::Patch, &url, body)
        .map_err(|error| match error.downcast::<Failure>() {
            // A blocking policy (reviewers, a build) refuses with a 403.
            Ok(failure) if failure.status == Some(403) => {
                Failure::conflict(format!("pull request {id} cannot complete yet: {}", failure.message))
                    .hint(format!(
                        "agent-cli ado pr update {id} --autocomplete on  (it completes once its policies pass)"
                    ))
                    .into()
            }
            Ok(failure) => failure.into(),
            Err(error) => error,
        })?;
    // Azure DevOps answers with the merge queued; it lands, or stops on a
    // conflict or a policy, a moment later.
    let until = (Instant::now() + SETTLE).min(ctx.deadline());
    // Accepted (it holds the completion options) and not yet merged.
    let queued = |pr: &serde_json::Value| {
        pr["status"].as_str() == Some("active")
            && match pr["mergeStatus"].as_str() {
                None | Some("queued" | "notSet") => true,
                Some("succeeded") => !pr["completionOptions"].is_null(),
                Some(_) => false,
            }
    };
    while queued(&done) && Instant::now() + POLL < until {
        std::thread::sleep(POLL);
        done = fetch_pr(ctx, &ado, id)?;
    }
    let row = pr_row(&ado, &done);
    if done["status"].as_str() == Some("completed") {
        return Ok(row);
    }
    let branch =
        |name: &str| text(&done[name]).map_or_else(String::new, |r| short_branch(&r).to_owned());
    let failure = match done["mergeStatus"].as_str() {
        _ if queued(&done) => {
            Failure::timed_out(format!("pull request {id}'s merge is still queued")).hint(format!(
                "agent-cli ado pr get {id} --fields status,merge_status"
            ))
        }
        Some("conflicts") => Failure::conflict(format!(
            "pull request {id} did not complete: {} and {} conflict",
            branch("sourceRefName"),
            branch("targetRefName")
        ))
        .hint(format!(
            "merge {} into {} and push, then run it again: agent-cli ado pr complete {id} --yes",
            branch("targetRefName"),
            branch("sourceRefName")
        )),
        Some("rejectedByPolicy") => Failure::conflict(format!(
            "pull request {id} did not complete: a branch policy refused the merge"
        ))
        .hint(format!("agent-cli ado pr get {id} --fields policies")),
        // Azure DevOps answers 200 and drops a completion that comes while
        // it is still working out a new pull request's merge.
        Some("succeeded") => Failure::conflict(format!(
            "pull request {id} is still active: Azure DevOps did not take the completion"
        ))
        .hint(format!(
            "agent-cli ado pr complete {id} --yes  (run it again)"
        )),
        merge => Failure::conflict(format!(
            "pull request {id} did not complete: its merge {}",
            merge.unwrap_or("did not run")
        ))
        .hint(format!("agent-cli ado pr get {id}")),
    };
    Err(failure.with_data(row).into())
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

    #[test]
    fn a_draft_is_refused_before_the_merge_and_a_conflict_after_it() {
        let (outcome, transport) = ado(
            &["ado", "pr", "complete", "17", "--yes"],
            vec![Answer::json(&pr(17, true))],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli ado pr update 17 --draft false, then run this again"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1, "nothing written");

        let mut queued = pr(17, false);
        queued["mergeStatus"] = json!("queued");
        let mut conflicted = pr(17, false);
        conflicted["mergeStatus"] = json!("conflicts");
        let (outcome, _) = ado(
            &["ado", "pr", "complete", "17", "--yes"],
            vec![
                Answer::json(&pr(17, false)),
                Answer::json(&queued),
                Answer::json(&conflicted),
            ],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert_eq!(outcome.json()["merge_status"], "conflicts");
        assert!(
            outcome.stderr.contains("pull request 17 did not complete: 42-fix-login and main conflict\nhint: merge main into 42-fix-login and push, then run it again: agent-cli ado pr complete 17 --yes"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_completion_azure_devops_dropped_says_to_run_it_again() {
        let (outcome, _) = ado(
            &["ado", "pr", "complete", "17", "--yes"],
            vec![Answer::json(&pr(17, false)), Answer::json(&pr(17, false))],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli ado pr complete 17 --yes  (run it again)"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_policy_that_blocks_is_exit_5_pointing_at_autocomplete() {
        let blocked = Answer::status(
            403,
            r#"{"message":"The pull request needs a minimum number of approvals (1) from other users before it can be completed."}"#,
        );
        let (outcome, _) = ado(
            &["ado", "pr", "complete", "17", "--yes"],
            vec![Answer::json(&pr(17, false)), blocked],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome.stderr.contains("minimum number of approvals"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("hint: agent-cli ado pr update 17 --autocomplete on  (it completes once its policies pass)"),
            "{}",
            outcome.stderr
        );
    }
}
