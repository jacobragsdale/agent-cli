use agent_cli_core::{Ctx, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Confluence, PAGE_MAX, note_more, text};

#[derive(clap::Args)]
pub struct SpaceListArgs {
    /// Only spaces whose key or name holds this, any case
    text: Option<String>,
    /// global (team spaces) or personal
    #[arg(long = "type", value_parser = PossibleValuesParser::new(["global", "personal"]), default_value = "global")]
    kind: String,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SpaceRow {
    /// The space key: what page list --space, page create --space and tree get take.
    id: String,
    name: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    /// The homepage's page id.
    homepage: Option<String>,
    url: Option<String>,
}

fn space_list(ctx: &Ctx, args: SpaceListArgs) -> Result<Vec<SpaceRow>> {
    let confluence = Confluence::load(ctx)?;
    let url = confluence.v2(
        "/spaces",
        &[
            ("type", args.kind),
            ("status", "current".to_owned()),
            ("sort", "key".to_owned()),
            ("limit", PAGE_MAX.to_string()),
        ],
    );
    let wanted = args.text.map(|text| text.trim().to_lowercase());
    // ponytail: a filter reads every space (250 a page); the v2 list has no
    // name search, and sites rarely hold more than a few thousand.
    let fetch = if wanted.is_some() {
        usize::MAX
    } else {
        args.limit
    };
    let (spaces, more) = confluence.list(ctx, url, fetch)?;
    let mut rows: Vec<SpaceRow> = spaces
        .iter()
        .filter(|space| {
            wanted.as_deref().is_none_or(|wanted| {
                [&space["key"], &space["name"]].iter().any(|field| {
                    field
                        .as_str()
                        .is_some_and(|value| value.to_lowercase().contains(wanted))
                })
            })
        })
        .filter_map(|space| {
            Some(SpaceRow {
                id: text(&space["key"])?,
                name: text(&space["name"]),
                kind: text(&space["type"]),
                homepage: text(&space["homepageId"]),
                url: confluence.web_of(space),
            })
        })
        .collect();
    let cut = rows.len() > args.limit;
    rows.truncate(args.limit);
    note_more(ctx, rows.len(), None, cut || (more && wanted.is_none()));
    Ok(rows)
}

command! {
    pub SPACE_LIST = ["confluence", "space", "list"], Read,
    "List Confluence spaces, or find one by key or name",
    keywords: ["wiki", "spaces", "team", "key", "find"],
    example: "confluence space list eng --fields id,name,homepage",
    run: space_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{V2, confluence, space, urls};

    #[test]
    fn spaces_are_filtered_by_key_or_name_across_pages() {
        let first = json!({"results": [space()],
            "_links": {"next": "/wiki/api/v2/spaces?cursor=abc&limit=250"}});
        let second = json!({"results": [{"id": "2002", "key": "OPS", "name": "Operations",
            "type": "global", "homepageId": "3000", "_links": {"webui": "/spaces/OPS"}}], "_links": {}});
        let (outcome, transport) = confluence(
            &["confluence", "space", "list", "engin"],
            vec![Answer::json(&first), Answer::json(&second)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "ENG", "name": "Engineering", "type": "global", "homepage": "1000",
                "url": "https://contoso.atlassian.net/wiki/spaces/ENG"}])
        );
        assert_eq!(
            urls(&transport),
            [
                format!("{V2}/spaces?type=global&status=current&sort=key&limit=250"),
                "https://contoso.atlassian.net/wiki/api/v2/spaces?cursor=abc&limit=250".to_owned()
            ]
        );
    }
}
