//! `confluence page diff`: a page's text against an earlier version of
//! itself (the one before, or `--version N`), or two pages or versions, as
//! Markdown by lines, with the title.

use std::collections::HashMap;

use agent_cli_core::{Ctx, Diff, DiffArgs, Failure, Rules, Side, command, diff};
use anyhow::Result;
use serde_json::json;

use crate::client::{Confluence, text};
use crate::ids;
use crate::markdown::{Context, to_markdown};
use crate::storage::parse;

#[derive(clap::Args)]
pub struct PageDiffArgs {
    /// The page: its id, ID@VERSION, KEY:Title or its URL
    page: String,
    /// Another page or ID@VERSION; left out, PAGE's current text against --version
    right: Option<String>,
    /// With one page, the earlier version (version list shows them); the one before when left out
    #[arg(long)]
    version: Option<String>,
    #[command(flatten)]
    diff: DiffArgs,
}

/// Versions of one page are no two environments.
const RULES: Rules = Rules {
    infer: false,
    ..Rules::PLAIN
};

fn page_diff(ctx: &Ctx, args: PageDiffArgs) -> Result<Diff> {
    let confluence = Confluence::load(ctx)?;
    let read = |id: &str, version: Option<u64>| -> Result<Side> {
        // As page get asks, so one recording answers both.
        let mut query = vec![
            ("body-format", "storage".to_owned()),
            ("include-labels", "true".to_owned()),
        ];
        if let Some(version) = version {
            query.push(("version", version.to_string()));
        }
        let content = confluence.content(ctx, id, &query)?;
        let value = &content.value;
        let storage = value["body"]["storage"]["value"]
            .as_str()
            .unwrap_or_default();
        let names = HashMap::new();
        let body = to_markdown(
            &parse(storage),
            &Context {
                space: "",
                names: &names,
            },
        )
        .text;
        Ok(Side::new(
            format!("{id}@{}", content.version()),
            json!({"title": text(&value["title"]), "body": body}),
        ))
    };
    let (left, right) = match &args.right {
        Some(right) => {
            if args.version.is_some() {
                return Err(Failure::usage(
                    "--version picks the earlier version of one page; with two, give ID@VERSION",
                )
                .into());
            }
            let (left_id, left_version) = ids::page(ctx, &confluence, &args.page, None)?;
            let (right_id, right_version) = ids::page(ctx, &confluence, right, None)?;
            (
                read(&left_id, left_version)?,
                read(&right_id, right_version)?,
            )
        }
        None => {
            let (id, wanted) = ids::page(ctx, &confluence, &args.page, args.version.as_deref())?;
            let current = read(&id, None)?;
            let now: u64 = current
                .id
                .rsplit('@')
                .next()
                .and_then(|version| version.parse().ok())
                .unwrap_or(1);
            let earlier = wanted.unwrap_or(now.saturating_sub(1));
            if earlier == 0 {
                return Err(Failure::not_found(format!("page {id} has one version")).into());
            }
            (read(&id, Some(earlier))?, current)
        }
    };
    diff(ctx, &args.diff, &RULES, left, right)
}

command! {
    pub PAGE_DIFF = ["confluence", "page", "diff"], Read,
    "Compare a page with its text before an edit, or two pages, as Markdown by lines",
    keywords: ["changed", "edited", "previous", "before", "wiki"],
    example: "confluence page diff 1101 --version 2",
    run: page_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{V2, confluence, page, urls};

    #[test]
    fn a_page_compares_with_the_version_before_by_lines() {
        let (outcome, transport) = confluence(
            &["confluence", "page", "diff", "1101"],
            vec![
                Answer::json(&page(
                    "1101",
                    "Runbook",
                    4,
                    "<p>Retry the load.</p><p>Page the owner.</p>",
                )),
                Answer::json(&page(
                    "1101",
                    "Runbook",
                    3,
                    "<p>Retry the load.</p><p>Wait an hour.</p>",
                )),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            (got["left"].clone(), got["right"].clone()),
            (json!("1101@3"), json!("1101@4"))
        );
        let lines = got["changes"][0]["lines"].to_string();
        assert!(
            lines.contains("-Wait an hour.") && lines.contains("+Page the owner."),
            "{lines}"
        );
        assert_eq!(
            urls(&transport)[1],
            format!("{V2}/pages/1101?body-format=storage&include-labels=true&version=3")
        );
        let (outcome, _) = confluence(
            &[
                "confluence",
                "page",
                "diff",
                "1101",
                "1102",
                "--version",
                "2",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
