use std::collections::HashMap;

use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::client::{Confluence, PAGE_MAX, note_more, person, stamp, text};
use crate::ids::current_page;
use crate::markdown::{Context, to_markdown};
use crate::storage::parse;

#[derive(clap::Args)]
pub struct CommentListArgs {
    /// The page: its id, KEY:Title or its URL
    page: String,
    /// footer (below the page) or inline (on its text)
    #[arg(long, value_enum)]
    kind: Option<Kind>,
    /// Only inline comments not yet resolved
    #[arg(long)]
    open: bool,
    /// Only comments made since: 15m, 2h, 7d, a date
    #[arg(long)]
    since: Option<When>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Kind {
    Footer,
    Inline,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CommentRow {
    /// What page comment --reply-to and comment update take.
    id: String,
    /// footer or inline.
    kind: Option<String>,
    author: Option<String>,
    date: Option<String>,
    /// The comment this one replies to.
    parent: Option<String>,
    /// The page text an inline comment is on.
    selection: Option<String>,
    /// An inline comment's: open, reopened, resolved or dangling (its text is gone).
    resolution: Option<String>,
    /// Markdown, cut at 1,000 characters.
    body: Option<String>,
}

/// The most of a comment's body a row prints.
const BODY: usize = 1000;
/// ponytail: newest first means reading every comment: up to 1,000 a page.
const MOST: usize = 1000;

/// Footer and inline comments, replies included, in one read of the
/// page's comment tree, newest first: ids grow as comments are made. An
/// inline reply has its thread's resolution.
fn comment_list(ctx: &Ctx, args: CommentListArgs) -> Result<Vec<CommentRow>> {
    let confluence = Confluence::load(ctx)?;
    let id = current_page(ctx, &confluence, &args.page)?;
    // The page is read first so a wrong id is exit 4 with its hint.
    confluence.content(ctx, &id, "", None)?;
    let url = confluence.api(
        &format!("/content/{id}/child/comment"),
        &[
            ("depth", "all".to_owned()),
            (
                "expand",
                "body.storage,history,ancestors,extensions.inlineProperties,extensions.resolution"
                    .to_owned(),
            ),
            ("limit", PAGE_MAX.to_string()),
        ],
    );
    let (mut found, cut) = confluence.list(ctx, url, MOST)?;
    let resolutions: HashMap<String, String> = found
        .iter()
        .filter_map(|row| {
            Some((
                text(&row["id"])?,
                text(&row["extensions"]["resolution"]["status"])?,
            ))
        })
        .collect();
    found.sort_by_key(|row| {
        std::cmp::Reverse(text(&row["id"]).and_then(|id| id.parse::<u64>().ok()))
    });
    let names = HashMap::new();
    let context = Context {
        space: "",
        names: &names,
    };
    let since = args.since.map(|since| since.0);
    let mut rows: Vec<CommentRow> = found
        .iter()
        .filter(|found| {
            since.is_none_or(|since| created(found).is_some_and(|created| created >= since))
        })
        .filter_map(|found| row(found, &resolutions, &context))
        .filter(|row| {
            args.kind.is_none_or(|kind| {
                row.kind.as_deref()
                    == Some(match kind {
                        Kind::Footer => "footer",
                        Kind::Inline => "inline",
                    })
            })
        })
        .filter(|row| !args.open || matches!(row.resolution.as_deref(), Some("open" | "reopened")))
        .collect();
    let more = rows.len() > args.limit || cut;
    rows.truncate(args.limit);
    note_more(ctx, rows.len(), None, more);
    Ok(rows)
}

fn created(found: &Value) -> Option<OffsetDateTime> {
    let raw = found["history"]["createdDate"]
        .as_str()
        .or_else(|| found["version"]["when"].as_str())?;
    OffsetDateTime::parse(raw, &Rfc3339).ok()
}

fn row(
    found: &Value,
    resolutions: &HashMap<String, String>,
    context: &Context,
) -> Option<CommentRow> {
    let id = text(&found["id"])?;
    let kind = text(&found["extensions"]["location"]);
    // Ancestors run nearest first: the parent, then up to the thread's root.
    let thread: Vec<String> = found["ancestors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|ancestor| ancestor["type"] == "comment")
        .filter_map(|ancestor| text(&ancestor["id"]))
        .collect();
    let root = thread.last().unwrap_or(&id);
    let inline = kind.as_deref() == Some("inline");
    let storage = found["body"]["storage"]["value"]
        .as_str()
        .unwrap_or_default();
    let body = to_markdown(&parse(storage), context).text;
    Some(CommentRow {
        kind,
        author: person(&found["history"]["createdBy"]).or_else(|| person(&found["version"]["by"])),
        date: stamp(&found["history"]["createdDate"]).or_else(|| stamp(&found["version"]["when"])),
        parent: thread.first().cloned(),
        selection: inline
            .then(|| text(&found["extensions"]["inlineProperties"]["originalSelection"]))
            .flatten(),
        resolution: inline.then(|| resolutions.get(root).cloned()).flatten(),
        body: (!body.is_empty()).then(|| cut(&body)),
        id,
    })
}

fn cut(body: &str) -> String {
    if body.chars().count() <= BODY {
        return body.to_owned();
    }
    let mut short: String = body.chars().take(BODY).collect();
    short.push('\u{2026}');
    short
}

command! {
    pub COMMENT_LIST = ["confluence", "comment", "list"], Read,
    "List a page's footer and inline comments, newest first, with what each is on",
    keywords: ["discussion", "feedback", "unresolved", "open", "resolved", "inline", "replies", "threads"],
    example: "confluence comment list 1201 --open --fields id,author,selection,body",
    run: comment_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{API, SAM, confluence, page, urls, user};

    fn found(
        id: &str,
        location: &str,
        ancestors: &[&str],
        extra: serde_json::Value,
    ) -> serde_json::Value {
        let mut row = json!({"id": id, "type": "comment", "status": "current", "title": "Re: Runbook",
            "body": {"storage": {"value": "<p>only once the CRM fix has <strong>synced</strong></p>", "representation": "storage"}},
            "history": {"latest": true, "createdBy": user(SAM, "sam", "Sam Lee"), "createdDate": "2026-09-28T09:15:00.000Z"},
            "ancestors": ancestors.iter().map(|a| json!({"id": a, "type": "comment"})).collect::<Vec<_>>(),
            "extensions": {"location": location,
                "resolution": {"status": "open", "lastModifier": {"type": "anonymous"}, "lastModifiedDate": 0}}});
        if let Some(properties) = extra.as_object() {
            row["extensions"]["inlineProperties"] = json!(properties);
        }
        row
    }

    #[test]
    fn comments_carry_their_thread_selection_and_resolution() {
        let mut reply = found(
            "5003",
            "inline",
            &["5002"],
            json!({"originalSelection": "retry the load", "markerRef": "r1"}),
        );
        reply["extensions"]["resolution"]["status"] = json!("resolved");
        let tree = json!({"results": [
            found("5001", "footer", &[], json!(null)),
            found("5002", "inline", &[], json!({"originalSelection": "retry the load", "markerRef": "r1"})),
            reply],
            "start": 0, "limit": 200, "size": 3, "_links": {}});
        let (outcome, transport) = confluence(
            &["confluence", "comment", "list", "1101", "--open"],
            vec![
                Answer::json(&page("1101", "Runbook", 4, "")),
                Answer::json(&tree),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let body = "only once the CRM fix has **synced**";
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "5003", "kind": "inline", "author": "Sam Lee", "date": "2026-09-28T09:15:00Z",
                    "parent": "5002", "selection": "retry the load", "resolution": "open", "body": body},
                {"id": "5002", "kind": "inline", "author": "Sam Lee", "date": "2026-09-28T09:15:00Z",
                    "selection": "retry the load", "resolution": "open", "body": body}])
        );
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{API}/content/1101/child/comment?depth=all&expand=body.storage%2Chistory%2Cancestors%2Cextensions.inlineProperties%2Cextensions.resolution&limit=200"
            )
        );
    }

    #[test]
    fn since_keeps_the_comments_made_after_it() {
        let mut old = found("5001", "footer", &[], json!(null));
        old["history"]["createdDate"] = json!("2026-08-01T09:00:00.000Z");
        let tree =
            json!({"results": [old, found("5004", "footer", &[], json!(null))], "_links": {}});
        let (outcome, _) = confluence(
            &[
                "confluence",
                "comment",
                "list",
                "1101",
                "--since",
                "2026-09-01",
            ],
            vec![
                Answer::json(&page("1101", "Runbook", 4, "")),
                Answer::json(&tree),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 1, "{rows}");
        assert_eq!(rows[0]["id"], "5004");
    }
}
