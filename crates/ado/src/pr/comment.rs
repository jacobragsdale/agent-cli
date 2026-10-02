use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Ado, Kind, text};
use crate::compose::{CommentBody, with_mentions};
use crate::ids::FileId;

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
    /// Open it on a file's lines: REPO[@REF]:PATH[:LINE[-LINE]], as diff get prints a hunk's at
    #[arg(long)]
    at: Option<String>,
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
    let mut thread = json!({
        "comments": [{"parentCommentId": 0, "content": with_mentions(ctx, &ado, &body.markdown())?, "commentType": "text"}],
        "status": "active",
    });
    if let Some(at) = &args.at {
        thread["threadContext"] = thread_context(&ado, &pr, at)?;
    }
    let thread = ado.change(ctx, Effect::Write, Method::Post, &url, thread)?;
    Ok(PrCommented {
        pr: id,
        thread_id: thread["id"].as_i64(),
    })
}

/// Where on the pull request's files `at` puts the thread: its right side
/// (the change), on the lines the id names, else on the whole file.
fn thread_context(ado: &Ado, pr: &Value, at: &str) -> Result<Value> {
    let file = FileId::parse(ado, at, None, None)?;
    let repo = text(&pr["repository"]["name"]).unwrap_or_default();
    if !file.repo.eq_ignore_ascii_case(&repo) || file.path.is_empty() {
        return Err(Failure::usage(format!(
            "--at {at} is not a file in {repo}, the pull request's repository"
        ))
        .hint(format!(
            "agent-cli ado diff get {} --names-only",
            pr["pullRequestId"]
        ))
        .into());
    }
    let mut context = json!({"filePath": format!("/{}", file.path)});
    if let Some((first, last)) = file.lines {
        context["rightFileStart"] = json!({"line": first, "offset": 1});
        context["rightFileEnd"] = json!({"line": last, "offset": 1});
    }
    Ok(context)
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

    #[test]
    fn at_opens_the_thread_on_the_lines_a_file_id_names() {
        let plans = dry_run(
            &[
                "ado",
                "pr",
                "comment",
                "17",
                "Unbounded",
                "--at",
                "web@abc123:src/x.cs:42-44",
            ],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(
            plans[0]["body"]["threadContext"],
            json!({"filePath": "/src/x.cs", "rightFileStart": {"line": 42, "offset": 1},
                "rightFileEnd": {"line": 44, "offset": 1}})
        );
        let plans = dry_run(
            &[
                "ado",
                "pr",
                "comment",
                "17",
                "Why?",
                "--at",
                "web:README.md",
            ],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(
            plans[0]["body"]["threadContext"],
            json!({"filePath": "/README.md"})
        );
        let (outcome, transport) = ado(
            &["ado", "pr", "comment", "17", "x", "--at", "api:src/x.cs:1"],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(outcome.code, 2, "another repository's file: {outcome:?}");
        assert_eq!(transport.sent().len(), 1, "nothing was posted");
    }
}
