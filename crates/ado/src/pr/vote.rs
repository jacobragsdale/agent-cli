use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Ado, Kind};

use super::{active_pr, vote_word};

#[derive(Clone, Copy, clap::ValueEnum)]
enum Vote {
    Approve,
    Suggest,
    Wait,
    Reject,
    None,
}

#[derive(clap::Args)]
pub struct VoteArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// Your vote (none withdraws it)
    #[arg(value_enum)]
    vote: Vote,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Voted {
    id: i64,
    /// approved, suggestions, none, waiting or rejected.
    vote: &'static str,
}

fn pr_vote(ctx: &Ctx, args: VoteArgs) -> Result<Voted> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let value: i64 = match args.vote {
        Vote::Approve => 10,
        Vote::Suggest => 5,
        Vote::Wait => -5,
        Vote::Reject => -10,
        Vote::None => 0,
    };
    // Azure DevOps refuses an edit to a closed pull request (TF401181).
    let (_, repo_id) = active_pr(ctx, &ado, id)?;
    let me = ado.me(ctx)?;
    // Voting on a pull request you do not review adds you as a reviewer.
    let url = ado.code(
        &format!(
            "git/repositories/{repo_id}/pullrequests/{}/reviewers/{}",
            id, me.id
        ),
        "",
    );
    ado.change(
        ctx,
        Effect::Write,
        Method::Put,
        &url,
        json!({"vote": value}),
    )?;
    Ok(Voted {
        id,
        vote: vote_word(value),
    })
}

command! {
    pub PR_VOTE = ["ado", "pr", "vote"], Write,
    "Record your vote on a pull request: approve, suggest, reject or none",
    keywords: ["approve", "reject", "sign", "off", "lgtm", "reviewer"],
    example: "ado pr vote 42 approve",
    run: pr_vote,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, dry_run, me, pr};

    #[test]
    fn a_vote_is_put_under_your_own_id() {
        let plans = dry_run(
            &["ado", "pr", "vote", "17", "suggest"],
            vec![Answer::json(&pr(17, false)), me()],
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/git/repositories/r-1/pullrequests/17/reviewers/u-1?api-version=7.1")
        );
        assert_eq!(plans[0]["body"], json!({"vote": 5}));

        let (outcome, _) = ado(
            &["ado", "pr", "vote", "17", "approve"],
            vec![
                Answer::json(&pr(17, false)),
                me(),
                Answer::json(&json!({"id": "u-1", "vote": 10})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!({"id": 17, "vote": "approved"}));
        let (outcome, _) = ado(&["ado", "pr", "vote", "17", "lgtm"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_closed_pull_request_takes_no_vote_or_update() {
        let mut closed = pr(17, false);
        closed["status"] = json!("abandoned");
        for argv in [
            &["ado", "pr", "vote", "17", "approve"][..],
            &["ado", "pr", "update", "17", "--title", "New"][..],
        ] {
            let (outcome, transport) = ado(argv, vec![Answer::json(&closed)]);
            assert_eq!(outcome.code, 5, "{outcome:?}");
            assert!(
                outcome
                    .stderr
                    .contains("pull request 17 is already abandoned"),
                "{}",
                outcome.stderr
            );
            assert!(transport.sent().iter().all(|sent| sent.method.is_read()));
        }
    }
}
