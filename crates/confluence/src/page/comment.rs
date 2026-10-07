use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Failure, Method, command, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Confluence, text};
use crate::compose::{Target, to_storage};
use crate::ids::{comment, current_page};
use crate::markdown::mentions;
use crate::storage::{parse, text_of};

use super::people;

#[derive(clap::Args)]
pub struct PageCommentArgs {
    /// The page: its id, KEY:Title or its URL
    page: String,
    /// Markdown, or - to read stdin (64 KiB max)
    #[arg(allow_hyphen_values = true, required_unless_present = "text_file")]
    text: Option<String>,
    /// The comment from a Markdown file (64 KiB max)
    #[arg(long)]
    text_file: Option<PathBuf>,
    /// Reply to this comment (its id or URL), footer or inline
    #[arg(long)]
    reply_to: Option<String>,
    /// Comment inline on this text of the page, as it reads
    #[arg(long)]
    on: Option<String>,
    /// Which occurrence of --on's text, from 1, when the page has several
    #[arg(long)]
    r#match: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Posted {
    /// The comment's id: what --reply-to and comment update take.
    id: String,
    page: String,
    /// footer or inline.
    kind: String,
    url: Option<String>,
}

const LIMIT: usize = 64 * 1024;

fn page_comment(ctx: &Ctx, args: PageCommentArgs) -> Result<Posted> {
    let confluence = Confluence::load(ctx)?;
    let Some(said) = ctx.long_text(
        "text",
        args.text.as_deref(),
        args.text_file.as_deref(),
        Some(LIMIT),
    )?
    else {
        return Err(Failure::usage("the comment is missing").into());
    };
    if args.on.is_some() && args.reply_to.is_some() {
        return Err(Failure::usage(
            "a reply follows its comment's anchor: --on or --reply-to, not both",
        )
        .into());
    }
    if args.r#match.is_some() && args.on.is_none() {
        return Err(Failure::usage("--match picks an occurrence of --on's text").into());
    }
    let id = current_page(ctx, &confluence, &args.page)?;
    let content = confluence.content(ctx, &id, &[("body-format", "storage".to_owned())])?;
    let storage = content.value["body"]["storage"]["value"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let nodes = parse(&storage);
    let space_id = text(&content.value["spaceId"]).unwrap_or_default();
    let key = confluence.space_key(ctx, &space_id, &mut Default::default())?;
    let people = people(ctx, &confluence, &said.text, &mentions(&nodes))?;
    let body = json!({"representation": "storage",
        "value": to_storage(&said.text, &Target { space: &key, people: &people })});
    let container = if content.kind == "pages" {
        "pageId"
    } else {
        "blogPostId"
    };
    let (kind, request) = match (&args.reply_to, &args.on) {
        (Some(parent), _) => {
            let parent = comment(&confluence.site, parent)?;
            let kind = parent_kind(ctx, &confluence, &parent, &id)?;
            (kind, json!({"parentCommentId": parent, "body": body}))
        }
        (None, Some(on)) => {
            let count = text_of(&nodes).matches(on.as_str()).count() as u64;
            let index = match (count, args.r#match) {
                (0, _) => {
                    return Err(
                        Failure::not_found(format!("page {id} does not read {on:?}"))
                            .hint(format!("agent-cli confluence page get {id}"))
                            .into(),
                    );
                }
                (1, None) => 0,
                (_, None) => {
                    return Err(Failure::usage(format!(
                        "page {id} reads {on:?} {count} times: say which with --match 1 to {count}"
                    ))
                    .into());
                }
                (_, Some(n)) if n == 0 || n > count => {
                    return Err(Failure::usage(format!(
                        "--match {n}: page {id} reads {on:?} {count} times"
                    ))
                    .into());
                }
                (_, Some(n)) => n - 1,
            };
            let properties = json!({"textSelection": on, "textSelectionMatchCount": count,
                "textSelectionMatchIndex": index});
            let mut request = json!({"body": body, "inlineCommentProperties": properties});
            request[container] = json!(id);
            ("inline", request)
        }
        (None, None) => {
            let mut request = json!({"body": body});
            request[container] = json!(id);
            ("footer", request)
        }
    };
    let url = confluence.v2(&format!("/{kind}-comments"), &[]);
    let posted = confluence.change(ctx, Effect::Write, Method::Post, &url, Some(request))?;
    Ok(Posted {
        id: text(&posted["id"]).unwrap_or_default(),
        page: id,
        kind: kind.to_owned(),
        url: confluence.web_of(&posted),
    })
}

/// A reply goes where its comment is: footer and inline comments live
/// under different calls, and an inline reply posted as a footer one is a
/// bug other tools shipped.
fn parent_kind(
    ctx: &Ctx,
    confluence: &Confluence,
    parent: &str,
    page: &str,
) -> Result<&'static str> {
    for kind in ["footer", "inline"] {
        match confluence.get(
            ctx,
            &confluence.v2(&format!("/{kind}-comments/{parent}"), &[]),
        ) {
            Ok(found) => {
                let on = text(&found["pageId"]).or_else(|| text(&found["blogPostId"]));
                if on.as_deref().is_some_and(|on| on != page) {
                    return Err(Failure::usage(format!(
                        "comment {parent} is on page {}, not {page}",
                        on.unwrap_or_default()
                    ))
                    .hint(format!(
                        "agent-cli confluence comment list {page} --fields id,kind,body"
                    ))
                    .into());
                }
                return Ok(kind);
            }
            Err(error) if status_of(&error) == Some(404) => {}
            Err(error) => return Err(error),
        }
    }
    Err(
        Failure::not_found(format!("no comment {parent}, or you may not see it"))
            .hint(format!(
                "agent-cli confluence comment list {page} --fields id,kind,body"
            ))
            .into(),
    )
}

command! {
    pub PAGE_COMMENT = ["confluence", "page", "comment"], Write,
    "Comment on a page: at its foot, inline on some text (--on), or as a reply",
    keywords: ["reply", "note", "discuss", "feedback", "inline", "footer", "annotate", "remark"],
    example: "confluence page comment 1201 'Rolled out to prod at 21:34' --on 'v1.4.2'",
    run: page_comment,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{V2, confluence, dry_run, page, space};

    const BODY: &str = "<p>First retry the load once. Then retry the load again.</p>";

    fn read() -> Vec<Answer> {
        vec![
            Answer::json(&page("1101", "Runbook", 4, BODY)),
            Answer::json(&space()),
        ]
    }

    #[test]
    fn a_footer_comment_is_markdown_stored() {
        let plans = dry_run(
            &["confluence", "page", "comment", "1101", "Done **twice**"],
            read(),
        );
        assert_eq!(plans[0]["url"], format!("{V2}/footer-comments"));
        assert_eq!(
            plans[0]["body"],
            json!({"pageId": "1101", "body": {"representation": "storage", "value": "<p>Done <strong>twice</strong></p>"}})
        );
    }

    #[test]
    fn an_inline_comment_counts_its_text_and_needs_match_when_there_are_several() {
        let (outcome, _) = confluence(
            &[
                "confluence",
                "page",
                "comment",
                "1101",
                "why?",
                "--on",
                "retry the load",
            ],
            read(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("2 times: say which with --match 1 to 2"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = confluence(
            &[
                "confluence",
                "page",
                "comment",
                "1101",
                "why?",
                "--on",
                "never said",
            ],
            read(),
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");

        let plans = dry_run(
            &[
                "confluence",
                "page",
                "comment",
                "1101",
                "only once",
                "--on",
                "retry the load",
                "--match",
                "2",
            ],
            read(),
        );
        assert_eq!(plans[0]["url"], format!("{V2}/inline-comments"));
        assert_eq!(
            plans[0]["body"]["inlineCommentProperties"],
            json!({"textSelection": "retry the load", "textSelectionMatchCount": 2, "textSelectionMatchIndex": 1})
        );
    }

    #[test]
    fn a_reply_goes_where_its_comment_is() {
        let inline = json!({"id": "5002", "pageId": "1101", "resolutionStatus": "open"});
        let plans = dry_run(
            &[
                "confluence",
                "page",
                "comment",
                "1101",
                "agreed",
                "--reply-to",
                "5002",
            ],
            [
                read(),
                vec![Answer::status(404, "{}"), Answer::json(&inline)],
            ]
            .concat(),
        );
        assert_eq!(plans[0]["url"], format!("{V2}/inline-comments"));
        assert_eq!(plans[0]["body"]["parentCommentId"], "5002");
        let elsewhere = json!({"id": "5002", "pageId": "1201"});
        let (outcome, _) = confluence(
            &[
                "confluence",
                "page",
                "comment",
                "1101",
                "agreed",
                "--reply-to",
                "5002",
            ],
            [read(), vec![Answer::json(&elsewhere)]].concat(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
