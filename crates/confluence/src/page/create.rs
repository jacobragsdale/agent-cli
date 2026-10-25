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
    let (key, parent) = match (key, args.parent) {
        (None, None) => {
            return Err(Failure::usage("say where: --space KEY or --parent ID")
                .hint("agent-cli confluence space list")
                .into());
        }
        (Some(key), None) => {
            let space = confluence.space(ctx, &key)?;
            (
                text(&space["key"]).unwrap_or(key),
                text(&space["homepage"]["id"]),
            )
        }
        (key, Some(parent)) => {
            let parent = parent.to_string();
            let found = confluence.content(ctx, &parent, "space", None)?;
            let in_space = found.space().unwrap_or_default();
            if let Some(key) = key
                && !in_space.eq_ignore_ascii_case(&key)
            {
                return Err(Failure::usage(format!(
                    "page {parent} is in space {in_space}, not {key}"
                ))
                .hint("drop --space, or pick a parent in that space")
                .into());
            }
            (in_space, Some(parent))
        }
    };
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
        "type": "page",
        "title": title,
        "space": {"key": key},
        "body": {"storage": {"value": storage, "representation": "storage"}},
    });
    if let Some(parent) = &parent {
        request["ancestors"] = json!([{"id": parent}]);
    }
    let created = confluence
        .change(
            ctx,
            Effect::Write,
            Method::Post,
            &confluence.api("/content", &[]),
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
        let url = confluence.api(&format!("/content/{id}/label"), &[]);
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

    use crate::testing::{API, BASE, confluence, dry_run, page, space, urls};

    fn created() -> Answer {
        Answer::json(
            &json!({"id": "1401", "type": "page", "status": "current", "title": "Postmortem",
            "space": {"key": "ENG"}, "version": {"number": 1},
            "_links": {"webui": "/spaces/ENG/pages/1401/Postmortem", "base": BASE}}),
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
            vec![Answer::json(&page("1100", "Runbooks", 3, ""))],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(plans[0]["url"], format!("{API}/content"));
        assert_eq!(plans[0]["headers"]["Authorization"], "***");
        assert_eq!(
            plans[0]["body"],
            json!({"type": "page", "title": "Postmortem", "space": {"key": "ENG"},
                "ancestors": [{"id": "1100"}],
                "body": {"storage": {"value": "<h1>What broke</h1><p>The <em>load</em>.</p>", "representation": "storage"}}})
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
            vec![
                Answer::json(&space()),
                created(),
                Answer::json(&json!({"results": []})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "1401", "title": "Postmortem", "space": "ENG", "version": 1,
                "url": format!("{BASE}/spaces/ENG/pages/1401/Postmortem")})
        );
        let sent = transport.sent();
        assert_eq!(
            sent[1].body.as_ref().unwrap()["ancestors"],
            json!([{"id": "1000"}]),
            "a page with no parent goes under the space's homepage"
        );
        assert_eq!(sent[2].url, format!("{API}/content/1401/label"));
        assert_eq!(
            sent[2].body,
            Some(
                json!([{"prefix": "global", "name": "postmortem"}, {"prefix": "global", "name": "etl"}])
            )
        );
        assert_eq!(
            urls(&transport)[0],
            format!("{API}/space/ENG?expand=homepage")
        );
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
                Answer::json(&space()),
                Answer::status(
                    400,
                    r#"{"statusCode":400,"data":{"authorized":false,"valid":true,"allowedInReadOnlyMode":true,"errors":[],"successful":false},"message":"A page with this title already exists: A page already exists with the title Runbook: etl_nightly in the space with key ENG. Log referral number is 00000000-0000-4000-8000-000000000001","reason":"Bad Request"}"#,
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
