use std::collections::HashMap;

use agent_cli_core::{Ctx, When, command, now};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Confluence, PAGE_MAX, note_more, stamp, text};
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
/// CQL's comment search returns at most 50 a page when it carries bodies.
const SEARCH_PAGE: usize = 50;

/// Footer and inline comments in one CQL read, newest first. Resolution is
/// not among what the search can expand for every caller, so it comes from
/// v2's list of the page's inline comments; a reply has its thread's.
fn comment_list(ctx: &Ctx, args: CommentListArgs) -> Result<Vec<CommentRow>> {
    let confluence = Confluence::load(ctx)?;
    let id = current_page(ctx, &confluence, &args.page)?;
    // A search on a page that is not there finds nothing; the read says so.
    let content = confluence.content(ctx, &id, &[])?;
    let mut cql = format!("container = {id} and type = comment");
    if let Some(since) = args.since {
        let minutes = (now() - since.0).whole_minutes().max(0);
        cql.push_str(&format!(" and created >= now(\"-{minutes}m\")"));
    }
    cql.push_str(" order by created desc");
    // ponytail: --kind and --open filter after the search, which reads up
    // to 1,000 comments to find them.
    let filtered = args.kind.is_some() || args.open;
    let wanted = if filtered { 1000 } else { args.limit };
    let searched = confluence.search(
        ctx,
        "/content/search",
        &[
            ("cql", cql),
            (
                "expand",
                "body.storage,version,ancestors,extensions.inlineProperties".to_owned(),
            ),
            ("limit", wanted.clamp(1, SEARCH_PAGE).to_string()),
        ],
        wanted,
    )?;
    let inline = searched
        .rows
        .iter()
        .any(|row| row["extensions"]["location"] == "inline");
    let resolutions = if inline {
        resolutions(ctx, &confluence, &format!("/{}/{id}", content.kind))?
    } else {
        HashMap::new()
    };
    let names = HashMap::new();
    let context = Context {
        space: "",
        names: &names,
    };
    let mut rows: Vec<CommentRow> = searched
        .rows
        .iter()
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
    let more = rows.len() > args.limit || (searched.more && !filtered);
    rows.truncate(args.limit);
    note_more(ctx, rows.len(), None, more);
    Ok(rows)
}

/// Each root inline comment's resolution, by id.
fn resolutions(ctx: &Ctx, confluence: &Confluence, page: &str) -> Result<HashMap<String, String>> {
    let url = confluence.v2(
        &format!("{page}/inline-comments"),
        &[("limit", PAGE_MAX.to_string())],
    );
    let (rows, _) = confluence.list(ctx, url, usize::MAX)?;
    Ok(rows
        .iter()
        .filter_map(|row| Some((text(&row["id"])?, text(&row["resolutionStatus"])?)))
        .collect())
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
        // An unedited comment's version is its author and its time.
        author: text(&found["version"]["by"]["displayName"])
            .or_else(|| text(&found["version"]["by"]["publicName"])),
        date: stamp(&found["version"]["when"]),
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

    use crate::testing::{V1, V2, confluence, page, urls};

    fn found(
        id: &str,
        location: &str,
        ancestors: &[&str],
        extra: serde_json::Value,
    ) -> serde_json::Value {
        let mut row = json!({"id": id, "type": "comment",
            "body": {"storage": {"value": "<p>only once the CRM fix has <strong>synced</strong></p>"}},
            "version": {"by": {"displayName": "Sam Lee"}, "when": "2026-09-28T09:15:00.000Z"},
            "ancestors": ancestors.iter().map(|a| json!({"id": a, "type": "comment"})).collect::<Vec<_>>(),
            "extensions": {"location": location}});
        if let Some(properties) = extra.as_object() {
            row["extensions"]["inlineProperties"] = json!(properties);
        }
        row
    }

    #[test]
    fn comments_carry_their_thread_selection_and_resolution() {
        let search = json!({"results": [
            found("5003", "inline", &["5002"], json!({"originalSelection": "", "markerRef": ""})),
            found("5002", "inline", &[], json!({"originalSelection": "retry the load", "markerRef": "r1"})),
            found("5001", "footer", &[], json!(null))], "_links": {}});
        let inline = json!({"results": [{"id": "5002", "resolutionStatus": "open"}], "_links": {}});
        let (outcome, transport) = confluence(
            &["confluence", "comment", "list", "1101", "--open"],
            vec![
                Answer::json(&page("1101", "Runbook", 4, "")),
                Answer::json(&search),
                Answer::json(&inline),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let body = "only once the CRM fix has **synced**";
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "5003", "kind": "inline", "author": "Sam Lee", "date": "2026-09-28T09:15:00Z",
                    "parent": "5002", "resolution": "open", "body": body},
                {"id": "5002", "kind": "inline", "author": "Sam Lee", "date": "2026-09-28T09:15:00Z",
                    "selection": "retry the load", "resolution": "open", "body": body}])
        );
        assert_eq!(
            urls(&transport)[1..],
            [
                format!(
                    "{V1}/content/search?cql=container+%3D+1101+and+type+%3D+comment+order+by+created+desc&expand=body.storage%2Cversion%2Cancestors%2Cextensions.inlineProperties&limit=50"
                ),
                format!("{V2}/pages/1101/inline-comments?limit=250"),
            ]
        );
    }
}
