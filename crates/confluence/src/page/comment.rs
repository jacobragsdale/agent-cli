use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Failure, Method, command, now, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Confluence, INLINE, text};
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
    let content = confluence.content(ctx, &id, "body.storage,space", None)?;
    let nodes = parse(content.storage());
    let key = content.space().unwrap_or_default();
    let people = people(ctx, &confluence, &said.text, &mentions(&nodes))?;
    let storage = to_storage(
        &said.text,
        &Target {
            space: &key,
            people: &people,
        },
    );
    let footer = |ancestors: Value| {
        let mut request = json!({
            "type": "comment",
            "container": {"id": id, "type": content.kind},
            "body": {"storage": {"value": storage, "representation": "storage"}},
        });
        if !ancestors.is_null() {
            request["ancestors"] = ancestors;
        }
        request
    };
    let (kind, url, request) = match (&args.reply_to, &args.on) {
        (Some(parent), _) => {
            let parent = comment(&confluence.site, parent)?;
            match thread(ctx, &confluence, &parent, &id)? {
                Thread::Inline(root) => (
                    "inline",
                    confluence.url(
                        INLINE,
                        &format!("/comments/{root}/replies"),
                        &[("containerId", id.clone())],
                    ),
                    json!({"body": storage, "commentId": number(&root)}),
                ),
                Thread::Footer => (
                    "footer",
                    confluence.api("/content", &[]),
                    footer(json!([{"id": parent}])),
                ),
            }
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
            // The server finds the text and marks it; the highlights are
            // the browser's record of the selection for other open tabs,
            // and without them it marks the page but saves no comment.
            let fetched = (now().unix_timestamp_nanos() / 1_000_000) as i64;
            (
                "inline",
                confluence.url(INLINE, "/comments", &[]),
                json!({"containerId": number(&id), "body": storage, "originalSelection": on,
                    "numMatches": count, "matchIndex": index, "lastFetchTime": fetched,
                    "serializedHighlights": "[]"}),
            )
        }
        (None, None) => (
            "footer",
            confluence.api("/content", &[]),
            footer(Value::Null),
        ),
    };
    let posted = confluence.change(ctx, Effect::Write, Method::Post, &url, Some(request))?;
    Ok(Posted {
        id: text(&posted["id"])
            .or_else(|| posted["id"].as_u64().map(|id| id.to_string()))
            .unwrap_or_default(),
        page: id,
        kind: kind.to_owned(),
        url: confluence
            .web_of(&posted)
            .or_else(|| text(&posted["commentDateUrl"]).map(|path| confluence.web(&path))),
    })
}

/// Where a reply goes.
enum Thread {
    /// Under the inline comment that starts the thread: the plugin keeps
    /// inline replies flat, and a reply posted through v1 lands as a footer
    /// comment, a bug other tools shipped.
    Inline(String),
    Footer,
}

/// The thread `parent` is in, which must be on `page`.
fn thread(ctx: &Ctx, confluence: &Confluence, parent: &str, page: &str) -> Result<Thread> {
    let url = confluence.api(
        &format!("/content/{parent}"),
        &[("expand", "container,ancestors".to_owned())],
    );
    let missing = || -> anyhow::Error {
        Failure::not_found(format!("no comment {parent}, or you may not see it"))
            .hint(format!(
                "agent-cli confluence comment list {page} --fields id,kind,body"
            ))
            .into()
    };
    let found = match confluence.get(ctx, &url) {
        Ok(found) if found["type"] == "comment" => found,
        Ok(_) => return Err(missing()),
        Err(error) if status_of(&error) == Some(404) => return Err(missing()),
        Err(error) => return Err(error),
    };
    let on = text(&found["container"]["id"]).unwrap_or_default();
    if on != page {
        return Err(
            Failure::usage(format!("comment {parent} is on page {on}, not {page}"))
                .hint(format!(
                    "agent-cli confluence comment list {page} --fields id,kind,body"
                ))
                .into(),
        );
    }
    if found["extensions"]["location"] != "inline" {
        return Ok(Thread::Footer);
    }
    // Ancestors run nearest first, to the thread's root.
    let root = found["ancestors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|ancestor| ancestor["type"] == "comment")
        .filter_map(|ancestor| text(&ancestor["id"]))
        .next_back()
        .unwrap_or_else(|| parent.to_owned());
    Ok(Thread::Inline(root))
}

/// An id as the plugin takes it: a JSON number.
fn number(id: &str) -> Value {
    id.parse::<u64>().map_or_else(|_| json!(id), |id| json!(id))
}

command! {
    pub PAGE_COMMENT = ["confluence", "page", "comment"], Write,
    "Comment on a page: at its foot, inline on some text (--on), or as a reply",
    keywords: ["add", "reply", "note", "discuss", "feedback", "inline", "footer", "annotate", "remark"],
    example: "confluence page comment 1201 'Rolled out to prod at 21:34' --on 'v1.4.2'",
    run: page_comment,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{API, BASE, confluence, dry_run, page};

    const BODY: &str = "<p>First retry the load once. Then retry the load again.</p>";
    const INLINE: &str = "https://wiki.contoso.example/confluence/rest/inlinecomments/1.0";

    fn read() -> Vec<Answer> {
        vec![Answer::json(&page("1101", "Runbook", 4, BODY))]
    }

    #[test]
    fn a_footer_comment_is_markdown_stored() {
        let plans = dry_run(
            &["confluence", "page", "comment", "1101", "Done **twice**"],
            read(),
        );
        assert_eq!(plans[0]["url"], format!("{API}/content"));
        assert_eq!(
            plans[0]["body"],
            json!({"type": "comment", "container": {"id": "1101", "type": "page"},
                "body": {"storage": {"value": "<p>Done <strong>twice</strong></p>", "representation": "storage"}}})
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

        let posted = json!({"id": 5003, "body": "<p>only once</p>", "markerRef": "m1",
            "originalSelection": "retry the load", "commentDateUrl": "/spaces/ENG/pages/1101/Runbook?focusedCommentId=5003#comment-5003"});
        let (outcome, transport) = confluence(
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
            [read(), vec![Answer::json(&posted)]].concat(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "5003", "page": "1101", "kind": "inline",
                "url": format!("{BASE}/spaces/ENG/pages/1101/Runbook?focusedCommentId=5003#comment-5003")})
        );
        let sent = transport.sent();
        assert_eq!(sent[1].url, format!("{INLINE}/comments"));
        let body = sent[1].body.clone().unwrap();
        assert_eq!(
            (
                &body["containerId"],
                &body["originalSelection"],
                &body["numMatches"],
                &body["matchIndex"],
                &body["serializedHighlights"]
            ),
            (
                &json!(1101),
                &json!("retry the load"),
                &json!(2),
                &json!(1),
                &json!("[]")
            )
        );
        assert_eq!(body["body"], "<p>only once</p>");
    }

    #[test]
    fn a_reply_goes_where_its_comment_is() {
        let reply = json!({"id": "5004", "type": "comment", "container": {"id": "1101", "type": "page"},
            "extensions": {"location": "inline"},
            "ancestors": [{"id": "5002", "type": "comment"}]});
        let plans = dry_run(
            &[
                "confluence",
                "page",
                "comment",
                "1101",
                "agreed",
                "--reply-to",
                "5004",
            ],
            [read(), vec![Answer::json(&reply)]].concat(),
        );
        assert_eq!(
            plans[0]["url"],
            format!("{INLINE}/comments/5002/replies?containerId=1101")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"body": "<p>agreed</p>", "commentId": 5002})
        );

        let footer = json!({"id": "5001", "type": "comment", "container": {"id": "1101", "type": "page"},
            "extensions": {"location": "footer"}, "ancestors": []});
        let plans = dry_run(
            &[
                "confluence",
                "page",
                "comment",
                "1101",
                "agreed",
                "--reply-to",
                "5001",
            ],
            [read(), vec![Answer::json(&footer)]].concat(),
        );
        assert_eq!(plans[0]["url"], format!("{API}/content"));
        assert_eq!(plans[0]["body"]["ancestors"], json!([{"id": "5001"}]));

        let elsewhere = json!({"id": "5002", "type": "comment", "container": {"id": "1201", "type": "page"},
            "extensions": {"location": "inline"}});
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
