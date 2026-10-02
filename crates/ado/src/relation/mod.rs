//! Links between two work items. A link is held on both items, under the
//! link type's name for the far end: on a child, its parent is a
//! `Hierarchy-Reverse` ("Parent") relation; on a predecessor, the item it
//! blocks is a `Dependency-Forward` ("Successor") one. Writing one end makes
//! Azure DevOps write the other, so these commands only touch item ID.

pub(crate) mod create;
pub(crate) mod delete;

use agent_cli_core::Ctx;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{API, Ado, Kind, list};

/// Exactly one kind of link, naming the other work item.
#[derive(clap::Args)]
#[group(required = true, multiple = false)]
struct LinkKind {
    /// The work item ID goes under
    #[arg(long)]
    parent: Option<i64>,
    /// The work item that goes under ID
    #[arg(long)]
    child: Option<i64>,
    /// A work item related to ID
    #[arg(long)]
    related: Option<i64>,
    /// The work item ID blocks (ID must finish first)
    #[arg(long)]
    blocks: Option<i64>,
    /// The work item that blocks ID (it must finish first)
    #[arg(long)]
    blocked_by: Option<i64>,
    /// The original that ID duplicates
    #[arg(long)]
    duplicate_of: Option<i64>,
}

impl LinkKind {
    /// `(kind as printed, relation type held on ID, the other item)`.
    fn pick(&self) -> (&'static str, &'static str, i64) {
        [
            ("parent", "System.LinkTypes.Hierarchy-Reverse", self.parent),
            ("child", "System.LinkTypes.Hierarchy-Forward", self.child),
            ("related", "System.LinkTypes.Related", self.related),
            ("blocks", "System.LinkTypes.Dependency-Forward", self.blocks),
            (
                "blocked-by",
                "System.LinkTypes.Dependency-Reverse",
                self.blocked_by,
            ),
            (
                "duplicate-of",
                "System.LinkTypes.Duplicate-Reverse",
                self.duplicate_of,
            ),
        ]
        .into_iter()
        .find_map(|(kind, rel, other)| Some((kind, rel, other?)))
        .expect("clap requires exactly one kind")
    }
}

/// What a create or delete did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Linked {
    work_item: i64,
    /// parent, child, related, blocks, blocked-by or duplicate-of: the flag given.
    link: &'static str,
    other: i64,
    /// create: the link was there already; nothing was written.
    already_linked: Option<bool>,
    /// delete: the link was removed.
    removed: Option<bool>,
}

/// Work item `raw` with its relations, and the revision a write tests.
fn read(ctx: &Ctx, ado: &Ado, raw: &str) -> Result<(i64, Value, i64)> {
    let id = ado.id(Kind::WorkItem, raw)?;
    let item = ado.get(
        ctx,
        &ado.api(
            None,
            &format!("wit/workitems/{id}"),
            "$expand=relations",
            API,
        ),
    )?;
    let rev = item["rev"]
        .as_i64()
        .context("the work item came back without a revision to test")?;
    Ok((id, item, rev))
}

/// Where in `item`'s relations a `rel` link to `other` sits: what
/// `remove /relations/N` names.
fn position(item: &Value, rel: &str, other: i64) -> Option<usize> {
    list(&item["relations"])
        .iter()
        .position(|relation| relation["rel"] == rel && linked_id(&relation["url"]) == Some(other))
}

/// The work item a relation's URL names: `…/_apis/wit/workItems/43`.
fn linked_id(url: &Value) -> Option<i64> {
    url.as_str()?.rsplit('/').next()?.parse().ok()
}

fn item_url(ado: &Ado, id: i64) -> String {
    ado.api(None, &format!("wit/workitems/{id}"), "", API)
}
