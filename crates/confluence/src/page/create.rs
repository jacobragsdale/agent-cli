use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Failure, Method, command, status_of};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::{Confluence, text};
use crate::compose::{Target, to_storage};
use crate::ids::{quote, space_key};

use super::{BODY_LIMIT, Written, labels, people};

#[derive(clap::Args)]
pub struct PageCreateArgs {
    /// The new page's title (unique in its space)
    #[arg(long)]
    title: String,
    /// The space, by key or URL (default: the parent's)
    #[arg(long)]
    space: Option<String>,
    /// The parent page's id (default: the space's homepage)
    #[arg(long)]
    parent: Option<u64>,
    /// The body: Markdown, or - to read stdin
    #[arg(long, allow_hyphen_values = true)]
    body: Option<String>,
    /// The body from a Markdown file
    #[arg(long)]
    body_file: Option<PathBuf>,
    /// The body is storage XHTML, sent as it is
    #[arg(long)]
    storage: bool,
    /// Labels to add, comma-separated
    #[arg(long)]
    labels: Option<String>,
}

fn page_create(ctx: &Ctx, args: PageCreateArgs) -> Result<Written> {
    let confluence = Confluence::load(ctx)?;
    let body = ctx.long_text(
        "body",
        args.body.as_deref(),
        args.body_file.as_deref(),
        Some(BODY_LIMIT),
    )?;
    let title = args.title.trim().to_owned();
    if title.is_empty() {
        return Err(Failure::usage("--title is blank").into());
    }
    let key = args
        .space
        .as_deref()
        .map(|space| space_key(&confluence.site, space))
        .transpose()?;
    let (space, parent) = match (key, args.parent) {
        (None, None) => {
            return Err(Failure::usage("say where: --space KEY or --parent ID")
                .hint("agent-cli confluence space list")
                .into());
        }
        (Some(key), None) => (confluence.space(ctx, &key)?, None),
        (key, Some(parent)) => {
            let parent = parent.to_string();
            let found = confluence.content(ctx, &parent, &[])?;
            let space_id = text(&found.value["spaceId"]).unwrap_or_default();
            let space = confluence.get(ctx, &confluence.v2(&format!("/spaces/{space_id}"), &[]))?;
            if let Some(key) = key
                && !text(&space["key"]).is_some_and(|known| known.eq_ignore_ascii_case(&key))
            {
                return Err(Failure::usage(format!(
                    "page {parent} is in space {}, not {key}",
                    text(&space["key"]).unwrap_or_default()
                ))
                .hint("drop --space, or pick a parent in that space")
                .into());
            }
            (space, Some(parent))
        }
    };
    let key = text(&space["key"]).unwrap_or_default();
    let storage = match &body {
        None => String::new(),
        Some(body) if args.storage => body.text.clone(),
        Some(body) => {
            let people = people(ctx, &confluence, &body.text, &[])?;
            to_storage(
                &body.text,
                &Target {
                    space: &key,
                    people: &people,
                },
            )
        }
    };
    let mut request = json!({
        "spaceId": text(&space["id"]),
        "status": "current",
        "title": title,
        "body": {"representation": "storage", "value": storage},
    });
    if let Some(parent) = &parent {
        request["parentId"] = json!(parent);
    }
    let created = confluence
        .change(
            ctx,
            Effect::Write,
            Method::Post,
            &confluence.v2("/pages", &[]),
            Some(request),
        )
        .map_err(|error| duplicate(error, &key, &title))?;
    let id = text(&created["id"]).unwrap_or_default();
    let wanted = args.labels.as_deref().map(labels).unwrap_or_default();
    if !wanted.is_empty() {
        let names: Vec<Value> = wanted
            .iter()
            .map(|name| json!({"prefix": "global", "name": name}))
            .collect();
        let url = confluence.v1(&format!("/content/{id}/label"), &[]);
        confluence.change(
            ctx,
            Effect::Write,
            Method::Post,
            &url,
            Some(Value::Array(names)),
        )?;
    }
    Ok(Written {
        title: text(&created["title"]),
        space: Some(key),
        version: created["version"]["number"].as_u64(),
        url: confluence.web_of(&created),
        id,
    })
}

/// A title already in the space (archived pages keep theirs) is a 400; it
/// is a conflict, and the page that holds it is one command away.
fn duplicate(error: anyhow::Error, key: &str, title: &str) -> anyhow::Error {
    let said = format!("{error:#}").to_lowercase();
    if status_of(&error) == Some(400) && said.contains("already exists") {
        return Failure::conflict(format!("space {key} already has a page titled {title:?}"))
            .hint(format!(
                "agent-cli confluence page get {}",
                quote(&format!("{key}:{title}"))
            ))
            .into();
    }
    error
}

command! {
    pub PAGE_CREATE = ["confluence", "page", "create"], Write,
    "Publish a new page from Markdown (or storage) in a space, under a parent",
    keywords: ["new", "add", "write", "publish", "post", "document", "postmortem"],
    example: "confluence page create --title 'Postmortem: etl_nightly' --parent 1100 --body-file postmortem.md",
    run: page_create,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{V1, V2, confluence, dry_run, page, space, spaces, urls};

    fn created() -> Answer {
        Answer::json(
            &json!({"id": "1401", "title": "Postmortem", "spaceId": "2001", "version": {"number": 1},
            "_links": {"webui": "/spaces/ENG/pages/1401/Postmortem"}}),
        )
    }

    #[test]
    fn a_page_is_made_from_markdown_under_its_parent_and_labelled() {
        let plans = dry_run(
            &[
                "confluence",
                "page",
                "create",
                "--title",
                "Postmortem",
                "--parent",
                "1100",
                "--body",
                "# What broke\n\nThe *load*.",
            ],
            vec![
                Answer::json(&page("1100", "Runbooks", 3, "")),
                Answer::json(&space()),
            ],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(plans[0]["url"], format!("{V2}/pages"));
        assert_eq!(plans[0]["headers"]["Authorization"], "***");
        assert_eq!(
            plans[0]["body"],
            json!({"spaceId": "2001", "status": "current", "title": "Postmortem", "parentId": "1100",
                "body": {"representation": "storage", "value": "<h1>What broke</h1><p>The <em>load</em>.</p>"}})
        );

        let (outcome, transport) = confluence(
            &[
                "confluence",
                "page",
                "create",
                "--title",
                "Postmortem",
                "--space",
                "ENG",
                "--labels",
                "postmortem, etl",
            ],
            vec![spaces(), created(), Answer::json(&json!({"results": []}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "1401", "title": "Postmortem", "space": "ENG", "version": 1,
                "url": "https://contoso.atlassian.net/wiki/spaces/ENG/pages/1401/Postmortem"})
        );
        let sent = transport.sent();
        assert_eq!(sent[2].url, format!("{V1}/content/1401/label"));
        assert_eq!(
            sent[2].body,
            Some(
                json!([{"prefix": "global", "name": "postmortem"}, {"prefix": "global", "name": "etl"}])
            )
        );
        assert_eq!(urls(&transport)[0], format!("{V2}/spaces?keys=ENG"));
    }

    #[test]
    fn a_title_the_space_holds_is_a_conflict_naming_the_page() {
        let (outcome, _) = confluence(
            &[
                "confluence",
                "page",
                "create",
                "--title",
                "Runbook: etl_nightly",
                "--space",
                "ENG",
            ],
            vec![
                spaces(),
                Answer::status(
                    400,
                    r#"{"errors":[{"status":400,"code":"INVALID_REQUEST_PARAMETER","title":"A page with this title already exists: A page already exists with the same TITLE in this space","detail":null}]}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli confluence page get 'ENG:Runbook: etl_nightly'"),
            "{}",
            outcome.stderr
        );
        let (outcome, transport) =
            confluence(&["confluence", "page", "create", "--title", "x"], vec![]);
        assert_eq!(outcome.code, 2);
        assert!(transport.sent().is_empty());
    }
}
