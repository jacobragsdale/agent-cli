use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{API, Ado, COMMENTS_API, Kind, stamp, text};
use crate::compose::{CommentBody, rich_text};

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
    // A comment goes under the item's own project, which may not be the
    // configured one. Reading it first also makes a deleted item exit 4,
    // where the comment alone answers a 500 about area permissions.
    let read = ado.api(
        None,
        &format!("wit/workitems/{id}"),
        "fields=System.TeamProject",
        API,
    );
    let project = text(&ado.get(ctx, &read)?["fields"]["System.TeamProject"]);
    let url = ado.api(
        Some(project.as_deref().unwrap_or(&ado.project)),
        &format!("wit/workItems/{}/comments", id),
        "",
        COMMENTS_API,
    );
    let html = rich_text(ctx, &ado, &body.markdown())?;
    let posted = ado.change(
        ctx,
        Effect::Write,
        Method::Post,
        &url,
        json!({"text": html}),
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

    use crate::testing::{BASE, ado, ado_piped, dry_run, home, page, person};

    #[test]
    fn comment_posts_markdown_as_html_to_the_preview_endpoint() {
        let plans = dry_run(
            &["ado", "workitem", "comment", "42", "Fixed in **!17**"],
            vec![home("Bug", "Fabrikam")],
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

        let (outcome, transport) = ado(
            &["ado", "workitem", "comment", "42", "done"],
            vec![
                home("Task", "Contoso Mobile"),
                Answer::json(
                    &json!({"id": 9001, "workItemId": 42, "createdDate": "2026-09-29T10:00:00Z"}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"work_item": 42, "id": 9001, "date": "2026-09-29T10:00:00Z"})
        );
        assert_eq!(
            transport.sent()[1].url,
            format!(
                "{BASE}/Contoso%20Mobile/_apis/wit/workItems/42/comments?api-version=7.1-preview.4"
            ),
            "an item in another project is commented on in its own"
        );
        let (outcome, _) = ado(&["ado", "workitem", "comment", "42", " "], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_flag_typed_where_the_text_goes_is_refused_not_posted() {
        for (argv, want) in [
            (
                &["ado", "workitem", "comment", "42", "--text=hello"][..],
                "unknown flag --text \u{2014} did you mean --text-file?",
            ),
            (
                &["ado", "workitem", "comment", "42", "--json"][..],
                "unknown flag --json",
            ),
            (
                &[
                    "ado",
                    "workitem",
                    "update",
                    "42",
                    "--state",
                    "Done",
                    "--comment",
                    "--if-rev=7",
                ][..],
                "--comment has no value of its own: --if-rev=7 came next",
            ),
            (
                &[
                    "ado",
                    "workitem",
                    "create",
                    "--type",
                    "Task",
                    "--title",
                    "t",
                    "--description",
                    "--parent=7",
                ][..],
                "--description has no value of its own: --parent=7 came next",
            ),
        ] {
            let (outcome, transport) = ado(argv, vec![]);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(outcome.stderr.contains(want), "{}", outcome.stderr);
            assert!(transport.sent().is_empty(), "nothing is sent: {argv:?}");
        }
        let plans = dry_run(
            &["ado", "workitem", "comment", "42", "-- a dash, not a flag"],
            vec![home("Bug", "Fabrikam")],
        );
        assert_eq!(
            plans[0]["body"],
            json!({"text": "<p>-- a dash, not a flag</p>"})
        );
    }

    #[test]
    fn a_piped_comment_is_a_code_block_a_file_is_markdown_and_both_stop_at_64_kib() {
        let (outcome, _) = ado_piped(
            "test result: FAILED. 1 passed; 1 failed\n",
            &["ado", "workitem", "comment", "42", "-", "--dry-run"],
            vec![home("Bug", "Fabrikam")],
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
            vec![home("Bug", "Fabrikam")],
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

    #[test]
    fn mentions_become_anchors_that_notify_and_an_unknown_one_sends_nothing() {
        let sam = || person("u-2", "Sam Lee", "sam@contoso.com");
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "comment",
                "42",
                "cc @<Sam Lee> and @sam@contoso.com, not jane@contoso.com or `@<x>`",
            ],
            vec![home("Bug", "Fabrikam"), sam(), sam()],
        );
        let anchor = r##"<a href="#" data-vss-mention="version:2.0,u-2">@Sam Lee</a>"##;
        assert_eq!(
            plans[0]["body"]["text"],
            format!(
                "<p>cc {anchor} and {anchor}, not jane@contoso.com or <code>@&lt;x&gt;</code></p>"
            )
        );

        let (outcome, transport) = ado(
            &["ado", "workitem", "comment", "42", "ask @<Nobody>"],
            vec![home("Bug", "Fabrikam"), page(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(outcome.stderr.contains("agent-cli ado person list"));
        assert!(transport.sent().iter().all(|sent| sent.method.is_read()));

        let (outcome, transport) = ado(
            &["ado", "workitem", "comment", "42", "ask @<Sam>"],
            vec![
                home("Bug", "Fabrikam"),
                page(vec![
                    json!({"id": "u-2", "providerDisplayName": "Sam Lee", "properties": {}}),
                    json!({"id": "u-3", "providerDisplayName": "Sam Leeds", "properties": {}}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().iter().all(|sent| sent.method.is_read()));
    }

    #[test]
    fn a_comment_on_a_deleted_work_item_is_not_found_not_a_permission_error() {
        let gone = agent_cli_core::testing::Answer::status(
            404,
            r#"{"message":"TF401232: Work item 904 does not exist, or you do not have permissions to read it."}"#,
        );
        let (outcome, transport) =
            crate::testing::ado(&["ado", "workitem", "comment", "904", "x"], vec![gone]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(transport.sent().iter().all(|sent| sent.method.is_read()));
        assert!(outcome.stderr.contains("TF401232"), "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains(
                "hint: agent-cli ado workitem list --text WORDS --fields id,title  (its id)"
            ),
            "{}",
            outcome.stderr
        );
    }
}
