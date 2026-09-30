use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Ado, Kind};
use crate::markdown::CommentBody;

use super::{fetch_pr, pr_home};

#[derive(clap::Args)]
pub struct PrCommentArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// Markdown, or - to read stdin (posted as a code block, 64 KiB max)
    #[arg(allow_hyphen_values = true, required_unless_present = "text_file")]
    text: Option<String>,
    /// The comment from a Markdown file (64 KiB max)
    #[arg(long)]
    text_file: Option<PathBuf>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrCommented {
    pr: i64,
    thread_id: Option<i64>,
}

fn pr_comment(ctx: &Ctx, args: PrCommentArgs) -> Result<PrCommented> {
    let body = CommentBody::read(ctx, args.text.as_deref(), args.text_file.as_deref())?;
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let pr = fetch_pr(ctx, &ado, id)?;
    let (repo_id, _) = pr_home(&pr)?;
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullRequests/{}/threads", id),
        "",
    );
    let thread = ado.change(
        ctx,
        Effect::Write,
        Method::Post,
        &url,
        json!({
            "comments": [{"parentCommentId": 0, "content": body.markdown(), "commentType": "text"}],
            "status": "active",
        }),
    )?;
    Ok(PrCommented {
        pr: id,
        thread_id: thread["id"].as_i64(),
    })
}

command! {
    pub PR_COMMENT = ["ado", "pr", "comment"], Write,
    "Start a comment thread on a pull request (Markdown, - for stdin, or --text-file)",
    keywords: ["discussion", "note", "reply", "post", "feedback", "review"],
    example: "ado pr comment 42 'Tests pass locally; ready for review'",
    run: pr_comment,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, dry_run, pr};

    #[test]
    fn a_comment_starts_a_thread_of_markdown() {
        let plans = dry_run(
            &["ado", "pr", "comment", "17", "LGTM, one nit"],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/git/repositories/r-1/pullRequests/17/threads?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"comments": [{"parentCommentId": 0, "content": "LGTM, one nit", "commentType": "text"}], "status": "active"})
        );
        let (outcome, _) = ado(
            &["ado", "pr", "comment", "17", "ok"],
            vec![
                Answer::json(&pr(17, false)),
                Answer::json(&json!({"id": 88, "status": "active"})),
            ],
        );
        assert_eq!(outcome.json(), json!({"pr": 17, "thread_id": 88}));
    }
}
