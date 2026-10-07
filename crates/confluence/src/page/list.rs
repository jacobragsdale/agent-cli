use agent_cli_core::{Ctx, Failure, When, command, now};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;

use crate::client::{Confluence, PAGE_MAX, note_more, stamp, text};
use crate::ids::space_key;
use crate::storage::{collapse, decode_entities};

#[derive(clap::Args)]
pub struct PageListArgs {
    /// Words to search for, ranked as the web search ranks them (the index trails edits by about a minute)
    text: Option<String>,
    /// Only in this space (its key or URL); repeatable
    #[arg(long)]
    space: Vec<String>,
    /// Only pages carrying this label; repeatable, each one required
    #[arg(long)]
    label: Vec<String>,
    /// Words in the title
    #[arg(long)]
    title: Option<String>,
    /// Only the direct children of this page id
    #[arg(long)]
    parent: Option<u64>,
    /// Created by this person: part of their name, or @me
    #[arg(long)]
    author: Option<String>,
    /// Only pages that mention you
    #[arg(long)]
    mentioned: bool,
    /// page or blogpost (default: both)
    #[arg(long = "type", value_parser = PossibleValuesParser::new(["page", "blogpost"]))]
    kind: Option<String>,
    /// Changed (or created, with --date created) since: 15m, 2h, 7d, a date
    #[arg(long)]
    since: Option<When>,
    /// Changed (or created) before this time
    #[arg(long)]
    until: Option<When>,
    /// What --since and --until look at
    #[arg(long, value_enum, default_value_t = Date::Changed)]
    date: Date,
    /// More CQL, ANDed with the rest (no order by)
    #[arg(long)]
    cql: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Clone, Copy, PartialEq, clap::ValueEnum)]
pub enum Date {
    Changed,
    Created,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PageRow {
    /// What page get takes.
    id: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    title: Option<String>,
    space: Option<String>,
    updated: Option<String>,
    updated_by: Option<String>,
    /// Where the words matched, cut at 150 characters.
    excerpt: Option<String>,
}

const EXCERPT: usize = 150;

fn page_list(ctx: &Ctx, args: PageListArgs) -> Result<Vec<PageRow>> {
    let confluence = Confluence::load(ctx)?;
    let spaces = args
        .space
        .iter()
        .map(|space| space_key(&confluence.site, space))
        .collect::<Result<Vec<_>>>()?;
    let cql = cql(&args, &spaces, now())?;
    let searched = confluence.search(
        ctx,
        "/search",
        &[
            ("cql", cql),
            ("expand", "content.space,content.version".to_owned()),
            ("limit", args.limit.clamp(1, PAGE_MAX).to_string()),
        ],
        args.limit,
    )?;
    let rows: Vec<PageRow> = searched.rows.iter().filter_map(row).collect();
    note_more(
        ctx,
        rows.len(),
        searched.total.map(|total| format!("~{total}")),
        searched.more,
    );
    Ok(rows)
}

fn row(found: &Value) -> Option<PageRow> {
    let content = &found["content"];
    // The result's own title is HTML-escaped and carries highlight marks.
    Some(PageRow {
        id: text(&content["id"])?,
        kind: text(&content["type"]),
        title: text(&content["title"]),
        space: text(&content["space"]["key"]),
        updated: stamp(&found["lastModified"]),
        updated_by: text(&content["version"]["by"]["displayName"])
            .or_else(|| text(&content["version"]["by"]["publicName"])),
        excerpt: text(&found["excerpt"]).map(|excerpt| cut(&excerpt)),
    })
}

/// The excerpt without highlight marks or entities, on one line.
fn cut(excerpt: &str) -> String {
    let plain = collapse(&decode_entities(
        &excerpt.replace("@@@hl@@@", "").replace("@@@endhl@@@", ""),
    ));
    if plain.chars().count() <= EXCERPT {
        return plain;
    }
    let mut short: String = plain.chars().take(EXCERPT).collect();
    short.push('\u{2026}');
    short
}

/// The CQL for a page search, built in one place. Every value is quoted and
/// escaped (`title="a" or type=page` is otherwise an injection). The text
/// clause leads, because `siteSearch` (how the web search ranks) is
/// silently dropped anywhere else, with `text ~` as its floor; the type is
/// pinned, since an untyped search mixes in attachments and comments. Times
/// are whole minutes before the server's now: CQL reads a date in the
/// caller's profile time zone and refuses RFC 3339.
fn cql(args: &PageListArgs, spaces: &[String], now: OffsetDateTime) -> Result<String> {
    let mut clauses = Vec::new();
    if let Some(text) = &args.text {
        let text = text.trim();
        if text.is_empty() {
            return Err(Failure::usage("the search text is blank")
                .hint("agent-cli confluence page list --space KEY")
                .into());
        }
        clauses.push(format!("siteSearch ~ {0} and text ~ {0}", quoted(text)));
    }
    clauses.push(match &args.kind {
        Some(kind) => format!("type = {kind}"),
        None => "type in (page, blogpost)".to_owned(),
    });
    if !spaces.is_empty() {
        let keys: Vec<String> = spaces.iter().map(|key| quoted(key)).collect();
        clauses.push(format!("space in ({})", keys.join(", ")));
    }
    for label in &args.label {
        clauses.push(format!("label = {}", quoted(label.trim())));
    }
    if let Some(title) = &args.title {
        clauses.push(format!("title ~ {}", quoted(title.trim())));
    }
    if let Some(parent) = args.parent {
        clauses.push(format!("parent = {parent}"));
    }
    match args.author.as_deref().map(str::trim) {
        Some("@me") => clauses.push("creator = currentUser()".to_owned()),
        Some(name) => clauses.push(format!("creator.fullname ~ {}", quoted(name))),
        None => {}
    }
    if args.mentioned {
        clauses.push("mention = currentUser()".to_owned());
    }
    let field = if args.date == Date::Created {
        "created"
    } else {
        "lastmodified"
    };
    for (operator, when) in [(">=", args.since), ("<=", args.until)] {
        if let Some(when) = when {
            let minutes = (now - when.0).whole_minutes();
            let offset = if minutes >= 0 {
                format!("-{minutes}m")
            } else {
                format!("+{}m", -minutes)
            };
            clauses.push(format!("{field} {operator} now(\"{offset}\")"));
        }
    }
    if let Some(raw) = &args.cql {
        if raw.to_ascii_lowercase().contains("order by") {
            return Err(Failure::usage("--cql may not hold an order by")
                .hint("drop the order by: a search is ranked by relevance, else newest first")
                .into());
        }
        clauses.push(format!("({})", raw.trim()));
    }
    let mut cql = clauses.join(" and ");
    if args.text.is_none() {
        cql.push_str(&format!(" order by {field} desc"));
    }
    Ok(cql)
}

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

command! {
    pub PAGE_LIST = ["confluence", "page", "list"], Read,
    "Search Confluence pages (runbooks, designs) by words, space, label, title, date",
    keywords: ["find", "search", "cql", "query", "recent", "changed", "label", "runbook", "documentation"],
    example: "confluence page list 'etl_nightly runbook' --space ENG --fields id,title,updated",
    run: page_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use clap::Parser;
    use serde_json::json;

    use super::*;
    use crate::testing::{V1, confluence, urls};

    #[derive(clap::Parser)]
    struct Line {
        #[command(flatten)]
        args: PageListArgs,
    }

    fn built(argv: &[&str]) -> String {
        let line = Line::try_parse_from([&["x"], argv].concat()).unwrap();
        let spaces: Vec<String> = line.args.space.clone();
        let now = OffsetDateTime::parse(
            "2026-09-29T12:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        cql(&line.args, &spaces, now).unwrap()
    }

    #[test]
    fn cql_leads_with_the_text_quotes_every_value_and_counts_minutes() {
        assert_eq!(
            built(&[
                "runbook \"etl\"",
                "--space",
                "ENG",
                "--label",
                "runbook",
                "--author",
                "@me"
            ]),
            r#"siteSearch ~ "runbook \"etl\"" and text ~ "runbook \"etl\"" and type in (page, blogpost) and space in ("ENG") and label = "runbook" and creator = currentUser()"#
        );
        assert_eq!(
            built(&[
                "--title",
                "a\" or type=page",
                "--since",
                "2026-09-22T12:00:00Z",
                "--date",
                "created",
                "--type",
                "page"
            ]),
            r#"type = page and title ~ "a\" or type=page" and created >= now("-10080m") order by created desc"#
        );
        assert_eq!(
            built(&["--parent", "1100", "--mentioned", "--cql", "label = x"]),
            "type in (page, blogpost) and parent = 1100 and mention = currentUser() and (label = x) order by lastmodified desc"
        );
    }

    #[test]
    fn a_search_reads_content_titles_and_cuts_excerpts() {
        let answer = json!({"results": [{
            "content": {"id": "1101", "type": "page", "title": "Runbook: etl_nightly",
                "space": {"key": "ENG"}, "version": {"by": {"displayName": "Jane Doe"}}},
            "title": "Runbook: @@@hl@@@etl_nightly@@@endhl@@@",
            "excerpt": "When @@@hl@@@etl_nightly@@@endhl@@@ fails &amp; the load stops", "lastModified": "2026-09-28T16:20:00.000Z"}],
            "totalSize": 590, "_links": {"next": "/rest/api/search?next=true&cursor=x&limit=1&cql=t"}});
        let (outcome, transport) = confluence(
            &["confluence", "page", "list", "etl_nightly", "--limit", "1"],
            vec![Answer::json(&answer)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "1101", "type": "page", "title": "Runbook: etl_nightly", "space": "ENG",
                "updated": "2026-09-28T16:20:00Z", "updated_by": "Jane Doe",
                "excerpt": "When etl_nightly fails & the load stops"}])
        );
        assert!(
            outcome.stderr.contains("[1 of ~590; --limit N]"),
            "{}",
            outcome.stderr
        );
        assert!(urls(&transport)[0].starts_with(&format!("{V1}/search?cql=siteSearch")));
        let (outcome, transport) = confluence(&["confluence", "page", "list", " "], vec![]);
        assert_eq!(outcome.code, 2, "a blank siteSearch is a 500 on the server");
        assert!(transport.sent().is_empty());
    }
}
