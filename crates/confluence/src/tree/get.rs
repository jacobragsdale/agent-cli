use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Confluence, PAGE_MAX, text};
use crate::ids::{PageRef, space_key};

#[derive(clap::Args)]
pub struct TreeGetArgs {
    /// A page (id, KEY:Title or URL) for what is below it, or a space (its key or URL) for its top
    target: String,
    /// How many levels down, 1 to 10
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=10))]
    depth: u8,
}

/// What sits below a page, or at the top of a space, as flat rows.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Tree {
    /// The page's id, or the space's key.
    id: String,
    title: Option<String>,
    space: Option<String>,
    nodes: Vec<Node>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Node {
    /// What page get takes, for a page.
    id: String,
    title: Option<String>,
    /// page, folder, whiteboard, database or embed.
    #[serde(rename = "type")]
    kind: Option<String>,
    parent: Option<String>,
    /// 1 is directly below.
    depth: u64,
}

/// ponytail: one tree read stops at 500 nodes; a lower --depth or a page
/// further down reads the rest.
const MAX_NODES: usize = 500;

fn tree_get(ctx: &Ctx, args: TreeGetArgs) -> Result<Tree> {
    let confluence = Confluence::load(ctx)?;
    let raw = args.target.trim();
    let page = match PageRef::parse(&confluence.site, raw) {
        Ok(PageRef::Space(_)) | Err(_) => None,
        Ok(page) => Some(page),
    };
    let mut tree = match page {
        Some(page) => {
            let (id, _) = page.resolve(ctx, &confluence)?;
            let found = confluence.content(ctx, &id, &[])?;
            let space_id = text(&found.value["spaceId"]).unwrap_or_default();
            let nodes = descendants(ctx, &confluence, &id, args.depth, 0)?;
            Tree {
                title: text(&found.value["title"]),
                space: Some(confluence.space_key(ctx, &space_id, &mut Default::default())?),
                id,
                nodes,
            }
        }
        None => {
            let key = space_key(&confluence.site, raw)?;
            let space = confluence.space(ctx, &key)?;
            let space_id = text(&space["id"]).unwrap_or_default();
            let home = text(&space["homepageId"]);
            let url = confluence.v2(
                &format!("/spaces/{space_id}/pages"),
                &[
                    ("depth", "root".to_owned()),
                    ("limit", PAGE_MAX.to_string()),
                ],
            );
            let (roots, _) = confluence.list(ctx, url, MAX_NODES + 1)?;
            let mut nodes = Vec::new();
            for root in &roots {
                let Some(id) = text(&root["id"]) else {
                    continue;
                };
                nodes.push(Node {
                    title: text(&root["title"]),
                    kind: Some("page".to_owned()),
                    parent: None,
                    depth: 1,
                    id: id.clone(),
                });
                if home.as_deref() == Some(id.as_str()) && args.depth > 1 {
                    nodes.extend(descendants(ctx, &confluence, &id, args.depth - 1, 1)?);
                }
            }
            Tree {
                id: text(&space["key"]).unwrap_or(key),
                title: text(&space["name"]),
                space: text(&space["key"]),
                nodes,
            }
        }
    };
    if tree.nodes.len() > MAX_NODES {
        tree.nodes.truncate(MAX_NODES);
        ctx.note(format!(
            "[{MAX_NODES} of {MAX_NODES}+; --depth 1, or a PAGE further down]"
        ));
    }
    Ok(tree)
}

/// A page's descendants in tree order (pages, folders, whiteboards,
/// databases and embeds alike), their depth counted from `offset`.
fn descendants(
    ctx: &Ctx,
    confluence: &Confluence,
    id: &str,
    depth: u8,
    offset: u64,
) -> Result<Vec<Node>> {
    let url = confluence.v2(
        &format!("/pages/{id}/descendants"),
        &[
            ("depth", depth.to_string()),
            ("limit", PAGE_MAX.to_string()),
        ],
    );
    let (rows, _) = confluence.list(ctx, url, MAX_NODES + 1)?;
    Ok(rows.iter().filter_map(|row| node(row, offset)).collect())
}

fn node(row: &Value, offset: u64) -> Option<Node> {
    Some(Node {
        id: text(&row["id"])?,
        title: text(&row["title"]),
        kind: text(&row["type"]),
        parent: text(&row["parentId"]),
        depth: row["depth"].as_u64().unwrap_or(1) + offset,
    })
}

command! {
    pub TREE_GET = ["confluence", "tree", "get"], Read,
    "Show the pages below a page, or at the top of a space, as flat rows",
    keywords: ["children", "child", "descendants", "hierarchy", "subpages", "under", "below", "structure", "navigate"],
    example: "confluence tree get 1100 --depth 1 --fields nodes",
    run: tree_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{V2, confluence, page, space, spaces, urls};

    #[test]
    fn a_page_tree_is_its_descendants_flat_in_tree_order() {
        let descendants = json!({"results": [
            {"id": "1101", "title": "Runbook: etl_nightly", "type": "page", "parentId": "1100", "depth": 1},
            {"id": "1102", "title": "Runbook: worker crash loop", "type": "page", "parentId": "1100", "depth": 1}],
            "_links": {}});
        let (outcome, transport) = confluence(
            &["confluence", "tree", "get", "1100"],
            vec![
                Answer::json(&page("1100", "Runbooks", 2, "")),
                Answer::json(&descendants),
                Answer::json(&space()),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "1100", "title": "Runbooks", "space": "ENG", "nodes": [
                {"id": "1101", "title": "Runbook: etl_nightly", "type": "page", "parent": "1100", "depth": 1},
                {"id": "1102", "title": "Runbook: worker crash loop", "type": "page", "parent": "1100", "depth": 1}]})
        );
        assert_eq!(
            urls(&transport)[1],
            format!("{V2}/pages/1100/descendants?depth=2&limit=250")
        );
    }

    #[test]
    fn a_space_tree_is_its_root_pages_and_its_homepage_below() {
        let roots = json!({"results": [{"id": "1000", "title": "Engineering"}, {"id": "1900", "title": "Scratch"}], "_links": {}});
        let below = json!({"results": [{"id": "1100", "title": "Runbooks", "type": "page", "parentId": "1000", "depth": 1}], "_links": {}});
        let (outcome, transport) = confluence(
            &["confluence", "tree", "get", "ENG"],
            vec![spaces(), Answer::json(&roots), Answer::json(&below)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let tree = outcome.json();
        assert_eq!(tree["id"], "ENG");
        assert_eq!(
            tree["nodes"][1],
            json!({"id": "1100", "title": "Runbooks", "type": "page", "parent": "1000", "depth": 2})
        );
        assert_eq!(tree["nodes"][2]["id"], "1900");
        assert_eq!(
            urls(&transport)[1..],
            [
                format!("{V2}/spaces/2001/pages?depth=root&limit=250"),
                format!("{V2}/pages/1000/descendants?depth=1&limit=250"),
            ]
        );
    }
}
