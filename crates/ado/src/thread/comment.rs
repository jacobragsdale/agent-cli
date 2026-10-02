use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Ado, text};
use crate::compose::{CommentBody, with_mentions};

use super::locate;

#[derive(clap::Args)]
pub struct ThreadCommentArgs {
    /// The thread: PR/THREAD (436/7) as thread list and pr get print it, or its web URL
    id: String,
    /// Markdown, or - to read stdin (posted as a code block, 64 KiB max)
    #[arg(allow_hyphen_values = true, required_unless_present = "text_file")]
    text: Option<String>,
    /// The reply from a Markdown file (64 KiB max)
    #[arg(long)]
    text_file: Option<PathBuf>,
    /// The pull request, when the id is the thread's number alone
    #[arg(long)]
    pr: Option<String>,
    /// Also resolve the thread (status fixed)
    #[arg(long)]
    resolve: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ThreadReplied {
    id: String,
    comment_id: Option<i64>,
    /// The thread's status, when --resolve changed it.
    status: Option<String>,
}

fn thread_comment(ctx: &Ctx, args: ThreadCommentArgs) -> Result<ThreadReplied> {
    let body = CommentBody::read(ctx, args.text.as_deref(), args.text_file.as_deref())?;
    let ado = Ado::load(ctx)?;
    let (id, path) = locate(ctx, &ado, &args.id, args.pr.as_deref())?;
    // A reply goes under the thread's first comment, as the web UI posts it.
    let content = with_mentions(ctx, &ado, &body.markdown())?;
    let reply = json!({"parentCommentId": 1, "content": content, "commentType": "text"});
    let url = ado.code(&format!("{path}/comments"), "");
    let comment = ado.change(ctx, Effect::Write, Method::Post, &url, reply)?;
    let mut status = None;
    if args.resolve {
        let url = ado.code(&path, "");
        let thread = ado.change(
            ctx,
            Effect::Write,
            Method::Patch,
            &url,
            json!({"status": "fixed"}),
        )?;
        status = text(&thread["status"]);
    }
    Ok(ThreadReplied {
        id,
        comment_id: comment["id"].as_i64(),
        status,
    })
}

command! {
    pub THREAD_COMMENT = ["ado", "thread", "comment"], Write,
    "Reply to a pull request review thread, and resolve it with --resolve",
    keywords: ["answer", "respond", "resolve", "fixed"],
    example: "ado thread comment 436/7 'Capped at 30 s in 9f1c2e4' --resolve",
    run: thread_comment,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, dry_run, person, pr, urls};

    #[test]
    fn a_reply_goes_under_the_first_comment_and_resolve_marks_it_fixed() {
        let plans = dry_run(
            &["ado", "thread", "comment", "17/7", "Capped", "--resolve"],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!(
                "{CODE}/git/repositories/r-1/pullRequests/17/threads/7/comments?api-version=7.1"
            )
        );
        assert_eq!(
            plans[0]["body"],
            json!({"parentCommentId": 1, "content": "Capped", "commentType": "text"})
        );

        let (outcome, transport) = ado(
            &[
                "ado",
                "thread",
                "comment",
                "7",
                "--pr",
                "17",
                "Capped",
                "--resolve",
            ],
            vec![
                Answer::json(&pr(17, false)),
                Answer::json(&json!({"id": 3, "parentCommentId": 1})),
                Answer::json(&json!({"id": 7, "status": "fixed"})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "17/7", "comment_id": 3, "status": "fixed"})
        );
        let sent = transport.sent();
        assert_eq!(sent[2].method.wire(), "PATCH");
        assert_eq!(
            urls(&transport)[2],
            format!("{CODE}/git/repositories/r-1/pullRequests/17/threads/7?api-version=7.1")
        );
    }

    #[test]
    fn a_thread_without_its_pull_request_is_exit_2() {
        let (outcome, transport) = ado(&["ado", "thread", "comment", "7", "ok"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn a_mention_in_a_reply_is_written_as_the_markdown_editor_writes_it() {
        let plans = dry_run(
            &["ado", "thread", "comment", "17/7", "@<Sam Lee> capped it"],
            vec![
                Answer::json(&pr(17, false)),
                person("u-2", "Sam Lee", "sam@contoso.com"),
            ],
        );
        assert_eq!(plans[0]["body"]["content"], "@<u-2> capped it");
    }
}
