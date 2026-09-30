use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Ado, COMMENTS_API, Kind, stamp};
use crate::markdown::CommentBody;

#[derive(clap::Args)]
pub struct CommentArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// Markdown, or - to read stdin (posted as a code block, 64 KiB max)
    #[arg(allow_hyphen_values = true, required_unless_present = "text_file")]
    text: Option<String>,
    /// The comment from a Markdown file (64 KiB max)
    #[arg(long)]
    text_file: Option<PathBuf>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CommentPosted {
    work_item: i64,
    /// The comment's id.
    id: Option<i64>,
    date: Option<String>,
}

fn workitem_comment(ctx: &Ctx, args: CommentArgs) -> Result<CommentPosted> {
    let body = CommentBody::read(ctx, args.text.as_deref(), args.text_file.as_deref())?;
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let url = ado.api(
        Some(&ado.project),
        &format!("wit/workItems/{}/comments", id),
        "",
        COMMENTS_API,
    );
    let posted = ado.change(
        ctx,
        Effect::Write,
        Method::Post,
        &url,
        json!({"text": body.html()}),
    )?;
    Ok(CommentPosted {
        work_item: id,
        id: posted["id"].as_i64(),
        date: stamp(&posted["createdDate"]),
    })
}

command! {
    pub WORKITEM_COMMENT = ["ado", "workitem", "comment"], Write,
    "Add a comment to a work item (Markdown, - for stdin, or --text-file)",
    keywords: ["note", "discussion", "reply", "post", "ticket"],
    example: "ado workitem comment 42 'Fixed in !17; deploying tomorrow'",
    run: workitem_comment,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, ado_piped, dry_run};

    #[test]
    fn comment_posts_markdown_as_html_to_the_preview_endpoint() {
        let plans = dry_run(
            &["ado", "workitem", "comment", "42", "Fixed in **!17**"],
            vec![],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{BASE}/Fabrikam/_apis/wit/workItems/42/comments?api-version=7.1-preview.4")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"text": "<p>Fixed in <b>!17</b></p>"})
        );

        let (outcome, _) = ado(
            &["ado", "workitem", "comment", "42", "done"],
            vec![Answer::json(
                &json!({"id": 9001, "workItemId": 42, "createdDate": "2026-09-29T10:00:00Z"}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"work_item": 42, "id": 9001, "date": "2026-09-29T10:00:00Z"})
        );
        let (outcome, _) = ado(&["ado", "workitem", "comment", "42", " "], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_piped_comment_is_a_code_block_a_file_is_markdown_and_both_stop_at_64_kib() {
        let (outcome, _) = ado_piped(
            "test result: FAILED. 1 passed; 1 failed\n",
            &["ado", "workitem", "comment", "42", "-", "--dry-run"],
            vec![],
        );
        assert_eq!(
            outcome.json()["would"][0]["body"],
            json!({"text": "<pre>test result: FAILED. 1 passed; 1 failed</pre>"})
        );

        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("note.md");
        std::fs::write(&note, "Fixed in **!17**\n").unwrap();
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "comment",
                "42",
                "--text-file",
                note.to_str().unwrap(),
            ],
            vec![],
        );
        assert_eq!(
            plans[0]["body"],
            json!({"text": "<p>Fixed in <b>!17</b></p>"})
        );

        let log = "x".repeat(64 * 1024 + 1);
        std::fs::write(&note, &log).unwrap();
        for (stdin, argv) in [
            (log.as_str(), &["ado", "workitem", "comment", "42", "-"][..]),
            (
                "",
                &[
                    "ado",
                    "workitem",
                    "comment",
                    "42",
                    "--text-file",
                    note.to_str().unwrap(),
                ][..],
            ),
        ] {
            let (outcome, transport) = ado_piped(stdin, argv, vec![]);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(
                outcome.stderr.contains("more than 64 KiB"),
                "{}",
                outcome.stderr
            );
            assert!(transport.sent().is_empty());
        }
        let (outcome, _) = ado(&["ado", "workitem", "comment", "42"], vec![]);
        assert_eq!(outcome.code, 2, "text or --text-file: {outcome:?}");
    }
}
