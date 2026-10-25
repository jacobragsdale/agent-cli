use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Confluence, PAGE_MAX, segment, text};
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
    let mut nodes = Vec::new();
    let mut tree = match page {
        Some(page) => {
            let (id, _) = page.resolve(ctx, &confluence)?;
            let found = confluence.content(ctx, &id, "space", None)?;
            below(ctx, &confluence, &id, args.depth, 1, &mut nodes)?;
            Tree {
                title: text(&found.value["title"]),
                space: found.space(),
                id,
                nodes,
            }
        }
        None => {
            let key = space_key(&confluence.site, raw)?;
            let space = confluence.space(ctx, &key)?;
            let key = text(&space["key"]).unwrap_or(key);
            let home = text(&space["homepage"]["id"]);
            let url = confluence.api(
                &format!("/space/{}/content/page", segment(&key)),
                &[
                    ("depth", "root".to_owned()),
                    ("limit", PAGE_MAX.to_string()),
                ],
            );
            let (roots, _) = confluence.list(ctx, url, MAX_NODES + 1)?;
            for root in &roots {
                let Some(id) = text(&root["id"]) else {
                    continue;
                };
                nodes.push(Node {
                    title: text(&root["title"]),
                    parent: None,
                    depth: 1,
                    id: id.clone(),
                });
                if home.as_deref() == Some(id.as_str()) && args.depth > 1 {
                    below(ctx, &confluence, &id, args.depth - 1, 2, &mut nodes)?;
                }
            }
            Tree {
                id: key.clone(),
                title: text(&space["name"]),
                space: Some(key),
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

/// The pages below `id`, `levels` deep, in tree order: each page's
/// children as the space orders them, then theirs. Data Center has no
/// descendants call for pages, so it is one child list a page with room
/// below it.
fn below(
    ctx: &Ctx,
    confluence: &Confluence,
    id: &str,
    levels: u8,
    depth: u64,
    nodes: &mut Vec<Node>,
) -> Result<()> {
    if levels == 0 || nodes.len() > MAX_NODES {
        return Ok(());
    }
    let url = confluence.api(
        &format!("/content/{id}/child/page"),
        &[("limit", PAGE_MAX.to_string())],
    );
    let (children, _) = confluence.list(ctx, url, MAX_NODES + 1)?;
    for child in &children {
        let Some(child_id) = text(&child["id"]) else {
            continue;
        };
        nodes.push(Node {
            title: text(&child["title"]),
            parent: Some(id.to_owned()),
            depth,
            id: child_id.clone(),
        });
        below(ctx, confluence, &child_id, levels - 1, depth + 1, nodes)?;
        if nodes.len() > MAX_NODES {
            break;
        }
    }
    Ok(())
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

    use crate::testing::{API, confluence, page, space, urls};

    fn children(ids: &[(&str, &str)]) -> Answer {
        let results: Vec<_> = ids
            .iter()
            .map(|(id, title)| json!({"id": id, "type": "page", "status": "current", "title": title}))
            .collect();
        Answer::json(
            &json!({"results": results, "start": 0, "limit": 200, "size": ids.len(), "_links": {}}),
        )
    }

    #[test]
    fn a_page_tree_is_its_descendants_flat_in_tree_order() {
        let (outcome, transport) = confluence(
            &["confluence", "tree", "get", "1100"],
            vec![
                Answer::json(&page("1100", "Runbooks", 2, "")),
                children(&[
                    ("1101", "Runbook: etl_nightly"),
                    ("1102", "Runbook: worker crash loop"),
                ]),
                children(&[("1105", "Backfill")]),
                children(&[]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "1100", "title": "Runbooks", "space": "ENG", "nodes": [
                {"id": "1101", "title": "Runbook: etl_nightly", "parent": "1100", "depth": 1},
                {"id": "1105", "title": "Backfill", "parent": "1101", "depth": 2},
                {"id": "1102", "title": "Runbook: worker crash loop", "parent": "1100", "depth": 1}]})
        );
        assert_eq!(
            urls(&transport)[1..],
            [
                format!("{API}/content/1100/child/page?limit=200"),
                format!("{API}/content/1101/child/page?limit=200"),
                format!("{API}/content/1102/child/page?limit=200"),
            ],
            "depth 2 asks no page at the bottom for its children"
        );
    }

    #[test]
    fn a_space_tree_is_its_root_pages_and_its_homepage_below() {
        let (outcome, transport) = confluence(
            &["confluence", "tree", "get", "ENG"],
            vec![
                Answer::json(&space()),
                children(&[("1000", "Engineering"), ("1900", "Scratch")]),
                children(&[("1100", "Runbooks")]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let tree = outcome.json();
        assert_eq!(tree["id"], "ENG");
        assert_eq!(
            tree["nodes"][1],
            json!({"id": "1100", "title": "Runbooks", "parent": "1000", "depth": 2})
        );
        assert_eq!(tree["nodes"][2]["id"], "1900");
        assert_eq!(
            urls(&transport)[1..],
            [
                format!("{API}/space/ENG/content/page?depth=root&limit=200"),
                format!("{API}/content/1000/child/page?limit=200"),
            ]
        );
    }
}
