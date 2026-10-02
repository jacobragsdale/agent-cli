//! Saved work item queries, My Queries and Shared Queries: the folder walk
//! `query list` prints and `query run` finds a path or a name in.

pub(crate) mod list;
pub(crate) mod run;

use agent_cli_core::Ctx;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Ado, list, text};

/// One saved query; folders are walked, never printed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct QueryRow {
    /// The query's GUID, which query run takes.
    id: String,
    name: String,
    /// Its folder path (`Shared Queries/Triage`), which query run also takes.
    path: String,
    /// flat, tree or oneHop: query run gives tree rows a parent, oneHop rows linked_from.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// In Shared Queries rather than My Queries.
    is_public: bool,
}

/// Azure DevOps walks at most two folder levels per request.
const WALK: &str = "$depth=2&$expand=minimal";

/// Every saved query the credential can see, in the folders' order.
pub(crate) fn queries(ctx: &Ctx, ado: &Ado) -> Result<Vec<QueryRow>> {
    let answer = ado.get(ctx, &ado.work("wit/queries", WALK))?;
    let mut rows = Vec::new();
    walk(ctx, ado, list(&answer["value"]), &mut rows)?;
    Ok(rows)
}

fn walk(ctx: &Ctx, ado: &Ado, items: &[Value], rows: &mut Vec<QueryRow>) -> Result<()> {
    for item in items {
        if item["isFolder"].as_bool() != Some(true) {
            rows.extend(row(item));
            continue;
        }
        let children = list(&item["children"]);
        match text(&item["id"]) {
            // Deeper than the request walked: the folder's own read goes on.
            Some(id) if children.is_empty() && item["hasChildren"].as_bool() == Some(true) => {
                let folder = ado.get(ctx, &ado.work(&format!("wit/queries/{id}"), WALK))?;
                walk(ctx, ado, list(&folder["children"]), rows)?;
            }
            _ => walk(ctx, ado, children, rows)?,
        }
    }
    Ok(())
}

fn row(item: &Value) -> Option<QueryRow> {
    Some(QueryRow {
        id: text(&item["id"])?,
        name: text(&item["name"])?,
        path: text(&item["path"])?,
        kind: text(&item["queryType"]).or_else(|| kind_of(item["wiql"].as_str()?)),
        is_public: item["isPublic"].as_bool().unwrap_or(false),
    })
}

/// `$expand=minimal` may leave out `queryType` but keeps the WIQL, which
/// says the same: links queried recursively are a tree.
fn kind_of(wiql: &str) -> Option<String> {
    let wiql = wiql.to_ascii_lowercase();
    let kind = if !wiql.contains("workitemlinks") {
        "flat"
    } else if wiql.contains("recursive") {
        "tree"
    } else {
        "oneHop"
    };
    Some(kind.to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    pub(crate) const TRIAGE: &str = "0d1f6a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b";
    pub(crate) const STORIES: &str = "1e2f3a4b-5c6d-4e7f-8a9b-0c1d2e3f4a5c";
    pub(crate) const MINE: &str = "2f3a4b5c-6d7e-4f8a-9b0c-1d2e3f4a5b6d";
    pub(crate) const DEEP: &str = "3a4b5c6d-7e8f-4a9b-8c0d-1e2f3a4b5c6e";

    /// My Queries holds a flat query; Shared Queries holds Triage and a
    /// folder whose children the two-level walk did not reach.
    pub(crate) fn tree() -> Answer {
        Answer::json(&json!({"count": 2, "value": [
            {"id": "f0000000-0000-4000-8000-000000000001", "name": "My Queries",
             "path": "My Queries", "isFolder": true, "hasChildren": true, "isPublic": false,
             "children": [{"id": MINE, "name": "Assigned to me", "path": "My Queries/Assigned to me",
                "isPublic": false,
                "wiql": "SELECT [System.Id] FROM WorkItems WHERE [System.AssignedTo] = @me"}]},
            {"id": "f0000000-0000-4000-8000-000000000002", "name": "Shared Queries",
             "path": "Shared Queries", "isFolder": true, "hasChildren": true, "isPublic": true,
             "children": [
                {"id": TRIAGE, "name": "Triage", "path": "Shared Queries/Triage",
                 "queryType": "flat", "isPublic": true},
                {"id": "f0000000-0000-4000-8000-000000000003", "name": "Web Team",
                 "path": "Shared Queries/Web Team", "isFolder": true, "hasChildren": true,
                 "isPublic": true}]}
        ]}))
    }

    /// The folder the first read stopped at.
    pub(crate) fn folder() -> Answer {
        Answer::json(
            &json!({"id": "f0000000-0000-4000-8000-000000000003", "name": "Web Team",
            "isFolder": true, "hasChildren": true, "children": [
            {"id": STORIES, "name": "Stories with tasks", "path": "Shared Queries/Web Team/Stories with tasks",
             "isPublic": true, "wiql": "SELECT [System.Id] FROM WorkItemLinks WHERE [System.Links.LinkType] = 'System.LinkTypes.Hierarchy-Forward' MODE (Recursive)"},
            {"id": DEEP, "name": "Triage", "path": "Shared Queries/Web Team/Triage",
             "queryType": "flat", "isPublic": true}]}),
        )
    }
}
