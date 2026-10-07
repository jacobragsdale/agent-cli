use std::collections::HashMap;

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Confluence, Content, stamp, text};
use crate::ids;
use crate::markdown::{Context, mentions, to_markdown};
use crate::storage::{find, parse, sections};

#[derive(clap::Args)]
pub struct PageGetArgs {
    /// The page: its id, ID@VERSION, KEY:Title or its URL. Several print an array, in order
    #[arg(required = true)]
    page: Vec<String>,
    /// An older version (version list shows them); ID@VERSION says the same
    #[arg(long)]
    version: Option<String>,
    /// Only this section: its heading's text, any case, through the next heading of its level
    #[arg(long)]
    section: Option<String>,
    /// Only these lines of the body: A-B, or one line with 20 either side
    #[arg(long)]
    line: Option<String>,
    /// The body as storage XHTML, not Markdown: what page update --storage takes back
    #[arg(long)]
    storage: bool,
}

/// One page, its body as Markdown and bounded.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PageText {
    /// What page get and page update take.
    id: String,
    /// page or blogpost.
    #[serde(rename = "type")]
    kind: String,
    title: Option<String>,
    space: Option<String>,
    /// current, or historical for an older version.
    status: Option<String>,
    /// What page update --if-version takes.
    version: u64,
    /// The parent page's id.
    parent: Option<String>,
    author: Option<String>,
    created: Option<String>,
    updated: Option<String>,
    updated_by: Option<String>,
    labels: Vec<String>,
    url: Option<String>,
    /// The body's lines shown: A-B.
    lines: Option<String>,
    /// The body's length in lines.
    total: usize,
    /// The h1 to h3 headings and their lines, when the body was cut.
    outline: Vec<Heading>,
    /// What the Markdown simplified (macros, layouts, comment marks); page
    /// update refuses to replace them with Markdown.
    lossy: Vec<String>,
    body: Option<String>,
    /// Where --output wrote the whole body.
    saved: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Heading {
    line: usize,
    level: usize,
    heading: String,
}

/// The output guard core cuts at; a body is bounded to fit under it.
const GUARD: usize = 12_000;
/// Lines either side of a single line asked for.
const AROUND: usize = 20;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Pages {
    One(Box<PageText>),
    Several(Vec<PageText>),
}

fn page_get(ctx: &Ctx, args: PageGetArgs) -> Result<Pages> {
    let confluence = Confluence::load(ctx)?;
    let mut fetched = Vec::new();
    let mut failed = Vec::new();
    let mut spaces = HashMap::new();
    for raw in &args.page {
        match fetch(ctx, &confluence, &args, raw, &mut spaces) {
            Ok(page) => fetched.push(page),
            Err(error) if args.page.len() == 1 => return Err(error),
            Err(error) => failed.push((raw, error)),
        }
    }
    // One users-bulk for every person the pages name.
    let ids: Vec<String> = fetched
        .iter()
        .flat_map(|page| {
            let mut ids = vec![
                text(&page.content.value["authorId"]).unwrap_or_default(),
                text(&page.content.value["version"]["authorId"]).unwrap_or_default(),
            ];
            ids.extend(mentions(&parse(&page.storage)));
            ids
        })
        .collect();
    let names = confluence.names(ctx, &ids);
    let budget = GUARD / args.page.len();
    let mut rows = Vec::new();
    for page in fetched {
        rows.push(render(ctx, &confluence, &args, page, &names, budget)?);
    }
    if let Some((_, first)) = failed.first() {
        let failure = first.downcast_ref::<Failure>();
        let message = failed
            .iter()
            .map(|(raw, error)| format!("{raw}: {error:#}"))
            .collect::<Vec<_>>()
            .join("; ");
        let exit = failure.map_or(agent_cli_core::Exit::Failed, |failure| failure.exit);
        let mut failure = Failure::new(exit, message).with_data(&rows);
        failure.hint = first.downcast_ref::<Failure>().and_then(|f| f.hint.clone());
        return Err(failure.into());
    }
    Ok(match rows.len() {
        1 if args.page.len() == 1 => Pages::One(Box::new(rows.remove(0))),
        _ => Pages::Several(rows),
    })
}

struct Fetched {
    content: Content,
    space: Option<String>,
    /// The storage shown: the whole body, or the section's.
    storage: String,
}

fn fetch(
    ctx: &Ctx,
    confluence: &Confluence,
    args: &PageGetArgs,
    raw: &str,
    spaces: &mut HashMap<String, String>,
) -> Result<Fetched> {
    let (id, version) = ids::page(ctx, confluence, raw, args.version.as_deref())?;
    let mut query = vec![
        ("body-format", "storage".to_owned()),
        ("include-labels", "true".to_owned()),
    ];
    if let Some(version) = version {
        query.push(("version", version.to_string()));
    }
    let content = confluence.content(ctx, &id, &query)?;
    let whole = content.value["body"]["storage"]["value"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let storage = match &args.section {
        Some(name) => {
            let nodes = parse(&whole);
            let all = sections(&whole, &nodes);
            whole[find(&all, name, &id)?.range.clone()].to_owned()
        }
        None => whole,
    };
    let space = match text(&content.value["spaceId"]) {
        Some(space_id) => Some(confluence.space_key(ctx, &space_id, spaces)?),
        None => None,
    };
    Ok(Fetched {
        content,
        space,
        storage,
    })
}

fn render(
    ctx: &Ctx,
    confluence: &Confluence,
    args: &PageGetArgs,
    page: Fetched,
    names: &HashMap<String, String>,
    budget: usize,
) -> Result<PageText> {
    let value = &page.content.value;
    let id = text(&value["id"]).unwrap_or_default();
    let name = |field: &serde_json::Value| {
        text(field).map(|account| names.get(&account).cloned().unwrap_or(account))
    };
    let read = to_markdown(
        &parse(&page.storage),
        &Context {
            space: page.space.as_deref().unwrap_or_default(),
            names,
        },
    );
    let body = if args.storage {
        page.storage.clone()
    } else {
        read.text
    };
    let mut row = PageText {
        id: id.clone(),
        kind: page.content.type_name().to_owned(),
        title: text(&value["title"]),
        space: page.space.clone(),
        status: text(&value["status"]),
        version: page.content.version(),
        parent: text(&value["parentId"]),
        author: name(&value["authorId"]),
        created: stamp(&value["createdAt"]),
        updated: stamp(&value["version"]["createdAt"]),
        updated_by: name(&value["version"]["authorId"]),
        labels: value["labels"]["results"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|label| text(&label["name"]))
            .collect(),
        url: confluence.web_of(value),
        lines: None,
        total: body.lines().count(),
        outline: Vec::new(),
        lossy: read.lossy,
        body: None,
        saved: None,
    };
    if let Some(saved) = ctx.save(body.as_bytes())? {
        row.saved = Some(saved.display().to_string());
        return Ok(row);
    }
    let lines: Vec<&str> = body.lines().collect();
    if lines.is_empty() {
        return Ok(row);
    }
    let (from, to) = match &args.line {
        Some(spec) => span(spec, lines.len())?,
        None => (1, lines.len()),
    };
    let mut shown = fit(&row, &lines, (from, to), budget);
    if shown < to {
        // The outline is part of what must fit, so the cut is made again.
        row.outline = outline(&lines);
        shown = fit(&row, &lines, (from, to), budget);
        ctx.note(format!(
            "[lines {from}-{shown} of {}; --section NAME, --line A-B, or --output FILE for all of it]",
            lines.len()
        ));
    }
    row.lines = Some(format!("{from}-{shown}"));
    row.body = Some(lines[from.saturating_sub(1).min(shown)..shown].join("\n"));
    Ok(row)
}

/// The last line, from `from`, that keeps the row within `budget` bytes of
/// JSON; at least the first, however long.
fn fit(row: &PageText, lines: &[&str], (from, to): (usize, usize), budget: usize) -> usize {
    let fixed = serde_json::to_string(row).map_or(0, |text| text.len());
    let mut room = budget.saturating_sub(fixed + 100);
    let mut shown = from - 1;
    for line in lines.iter().take(to).skip(from - 1) {
        // Quoted and escaped, the quotes stand for the `\n` that joins it.
        let size = serde_json::to_string(line).map_or(line.len(), |text| text.len());
        if size > room && shown >= from {
            break;
        }
        room = room.saturating_sub(size);
        shown += 1;
    }
    shown.max(from.min(lines.len()))
}

/// `A-B`, or `N` with [`AROUND`] lines either side, within the body.
fn span(spec: &str, total: usize) -> Result<(usize, usize)> {
    let wrong = || Failure::usage(format!("--line {spec}: give A-B or N within 1-{total}"));
    let parse = |raw: &str| raw.trim().parse::<usize>().ok().filter(|n| *n >= 1);
    let (from, to) = match spec.split_once('-') {
        Some((a, b)) => (parse(a).ok_or_else(wrong)?, parse(b).ok_or_else(wrong)?),
        None => {
            let line = parse(spec).ok_or_else(wrong)?;
            (line.saturating_sub(AROUND).max(1), line + AROUND)
        }
    };
    if from > to || from > total {
        return Err(wrong().into());
    }
    Ok((from, to.min(total)))
}

/// The h1 to h3 lines of Markdown, fenced code aside.
fn outline(lines: &[&str]) -> Vec<Heading> {
    let mut fenced = false;
    let mut found = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        let level = line.chars().take_while(|c| *c == '#').count();
        if !fenced && (1..=3).contains(&level) && line[level..].starts_with(' ') {
            found.push(Heading {
                line: at + 1,
                level,
                heading: line[level..].trim().to_owned(),
            });
        }
    }
    found
}

command! {
    pub PAGE_GET = ["confluence", "page", "get"], Read,
    "Show a page as Markdown with its version, author, labels and outline",
    keywords: ["read", "open", "body", "content", "section", "wiki", "document", "markdown", "storage"],
    example: "confluence page get 1101 --section 'An order without customer_id'",
    run: page_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{V2, confluence, page, people, space, urls};

    const BODY: &str = "<h2>Steps</h2><p>Fix the order in the CRM.</p><h2>An order without customer_id</h2><p>Retry <code>load_orders</code>.</p><ac:structured-macro ac:name=\"toc\" />";

    #[test]
    fn a_page_reads_as_markdown_with_who_changed_it_and_what_markdown_lost() {
        let (outcome, transport) = confluence(
            &[
                "confluence",
                "page",
                "get",
                "https://contoso.atlassian.net/wiki/spaces/ENG/pages/1101/Runbook",
            ],
            vec![
                Answer::json(&page("1101", "Runbook: etl_nightly", 4, BODY)),
                Answer::json(&space()),
                people(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "1101", "type": "page", "title": "Runbook: etl_nightly", "space": "ENG",
                "status": "current", "version": 4, "parent": "1100", "author": "Sam Lee",
                "created": "2026-09-01T09:00:00Z", "updated": "2026-09-28T16:20:00Z",
                "updated_by": "Jane Doe", "labels": ["runbook"],
                "url": "https://contoso.atlassian.net/wiki/spaces/ENG/pages/1101/Page",
                "lines": "1-9", "total": 9, "lossy": ["toc"],
                "body": "## Steps\n\nFix the order in the CRM.\n\n## An order without customer_id\n\nRetry `load_orders`.\n\n\u{27e6}toc\u{27e7}"})
        );
        assert_eq!(
            urls(&transport),
            [
                format!("{V2}/pages/1101?body-format=storage&include-labels=true"),
                format!("{V2}/spaces/2001"),
                format!("{V2}/users-bulk"),
            ]
        );
    }

    #[test]
    fn a_section_an_old_version_and_a_blog_post_are_asked_for_as_such() {
        let (outcome, transport) = confluence(
            &[
                "confluence",
                "page",
                "get",
                "1101@2",
                "--section",
                "an order WITHOUT customer_id",
            ],
            vec![
                Answer::status(404, r#"{"errors":[{"status":404,"title":"Not found"}]}"#),
                Answer::json(&page("1101", "Shipped", 2, BODY)),
                Answer::json(&space()),
                people(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = outcome.json();
        assert_eq!(row["type"], "blogpost");
        assert_eq!(
            row["body"],
            "## An order without customer_id\n\nRetry `load_orders`.\n\n\u{27e6}toc\u{27e7}"
        );
        assert_eq!(
            urls(&transport)[..2],
            [
                format!("{V2}/pages/1101?body-format=storage&include-labels=true&version=2"),
                format!("{V2}/blogposts/1101?body-format=storage&include-labels=true&version=2"),
            ]
        );
        let (outcome, _) = confluence(
            &["confluence", "page", "get", "1101@2", "--version", "3"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_long_body_prints_to_a_line_with_its_outline_and_output_saves_it_all() {
        let mut body = String::new();
        for section in 1..=40 {
            body.push_str(&format!("<h2>Part {section}</h2>"));
            for _ in 0..6 {
                body.push_str("<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor.</p>");
            }
        }
        let answers = || {
            vec![
                Answer::json(&page("1101", "Long", 1, &body)),
                Answer::json(&space()),
                people(),
            ]
        };
        let (outcome, _) = confluence(&["confluence", "page", "get", "1101"], answers());
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(outcome.stdout.len() <= 12_000, "{}", outcome.stdout.len());
        let row = outcome.json();
        assert_eq!(
            row["outline"][1],
            json!({"line": 15, "level": 2, "heading": "Part 2"})
        );
        assert!(
            outcome
                .stderr
                .contains("of 559; --section NAME, --line A-B, or --output FILE"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = confluence(
            &["confluence", "page", "get", "1101", "--line", "15"],
            answers(),
        );
        assert_eq!(outcome.json()["lines"], "1-35");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.md");
        let (outcome, _) = confluence(
            &[
                "confluence",
                "page",
                "get",
                "1101",
                "--output",
                path.to_str().unwrap(),
            ],
            answers(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["saved"], path.display().to_string());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .ends_with("sed do eiusmod tempor.")
        );
    }

    #[test]
    fn several_pages_print_in_order_and_a_missing_one_fails_with_the_rest_as_data() {
        let missing = || {
            Answer::status(
                404,
                r#"{"errors":[{"status":404,"title":"Cannot find a page with id [9]","detail":null}]}"#,
            )
        };
        let (outcome, _) = confluence(
            &["confluence", "page", "get", "1101", "9"],
            vec![
                Answer::json(&page("1101", "Runbook", 4, "<p>x</p>")),
                Answer::json(&space()),
                missing(),
                missing(),
                people(),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert_eq!(outcome.json()[0]["id"], "1101");
        assert!(
            outcome.stderr.contains("or you may not see it"),
            "{}",
            outcome.stderr
        );
    }
}
