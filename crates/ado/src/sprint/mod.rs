//! A team's sprints: the list, one sprint's load against its capacity, and
//! closing one by moving its unfinished work on. What they share: the sprint
//! row, the work items in a sprint, and "done" judged by each type's states.

pub(crate) mod complete;
pub(crate) mod get;
pub(crate) mod list;

use std::collections::HashMap;

use agent_cli_core::Ctx;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Ado, list, segment, text};
use crate::iteration::Iteration;
use crate::work_items::read;

const STATE: &str = "System.State";
const TYPE: &str = "System.WorkItemType";
const ITERATION: &str = "System.IterationPath";

/// A sprint as list and get print it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SprintRow {
    /// Its path, which sprint get and --iteration take.
    id: String,
    name: String,
    path: String,
    start: Option<String>,
    finish: Option<String>,
    /// past, current or future.
    timeframe: Option<String>,
}

impl From<Iteration> for SprintRow {
    fn from(sprint: Iteration) -> Self {
        Self {
            id: sprint.path.clone(),
            name: sprint.name,
            path: sprint.path,
            start: sprint.start,
            finish: sprint.finish,
            timeframe: sprint.timeframe,
        }
    }
}

/// The work items in `sprint` with `fields`, and each child's parent. The
/// iteration's list also names parents planned in other sprints, so only
/// items whose iteration is this sprint's path are kept.
fn items(
    ctx: &Ctx,
    ado: &Ado,
    team: &str,
    sprint: &Iteration,
    fields: &[&str],
) -> Result<(Vec<Value>, HashMap<i64, i64>)> {
    let url = ado.team(
        team,
        &format!(
            "work/teamsettings/iterations/{}/workitems",
            segment(&sprint.id)
        ),
        "",
    );
    let answer = ado.get(ctx, &url)?;
    let mut ids: Vec<i64> = Vec::new();
    let mut parents = HashMap::new();
    for relation in list(&answer["workItemRelations"]) {
        let Some(id) = relation["target"]["id"].as_i64() else {
            continue;
        };
        if let Some(parent) = relation["source"]["id"].as_i64() {
            parents.insert(id, parent);
        }
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let mut fields = fields.to_vec();
    fields.extend([TYPE, STATE, ITERATION]);
    let items = read(ctx, ado, &ids, &fields)?
        .into_iter()
        .filter(|item| {
            text(&item["fields"][ITERATION])
                .is_some_and(|path| path.eq_ignore_ascii_case(&sprint.path))
        })
        .collect();
    Ok((items, parents))
}

/// Whether `item` is finished, by its type's state categories, asking once
/// per type and state in a run (the states cache is off under --no-cache).
fn done(
    ctx: &Ctx,
    ado: &Ado,
    seen: &mut HashMap<(String, String), bool>,
    item: &Value,
) -> Result<bool> {
    let field = |name: &str| text(&item["fields"][name]).unwrap_or_default();
    let key = (field(TYPE), field(STATE));
    if let Some(done) = seen.get(&key) {
        return Ok(*done);
    }
    let done = crate::types::done(ctx, ado, &key.0, &key.1)?;
    seen.insert(key, done);
    Ok(done)
}

#[cfg(test)]
pub(crate) mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    /// Sprints 11 (past), 12 (current) and 13 (future) of the Web Team; 13
    /// is dated far ahead, so its days left do not depend on today.
    pub(crate) fn sprints() -> Answer {
        let sprint = |n: u32, start: &str, finish: &str, timeframe: &str| {
            json!({"id": format!("i-{n}"), "name": format!("Sprint {n}"),
                "path": format!("Fabrikam\\Sprint {n}"),
                "attributes": {"startDate": format!("{start}T00:00:00Z"),
                    "finishDate": format!("{finish}T00:00:00Z"), "timeFrame": timeframe}})
        };
        Answer::json(&json!({"count": 3, "value": [
            sprint(11, "2026-09-08", "2026-09-21", "past"),
            sprint(12, "2026-09-22", "2026-10-05", "current"),
            sprint(13, "2099-03-02", "2099-03-13", "future"),
        ]}))
    }

    /// The iteration's work items: `(id, parent)` pairs.
    pub(crate) fn relations(items: &[(i64, Option<i64>)]) -> Answer {
        let relations: Vec<Value> = items
            .iter()
            .map(|(id, parent)| match parent {
                Some(parent) => json!({"rel": "System.LinkTypes.Hierarchy-Forward",
                    "source": {"id": parent}, "target": {"id": id}}),
                None => json!({"rel": null, "source": null, "target": {"id": id}}),
            })
            .collect();
        Answer::json(&json!({"workItemRelations": relations}))
    }

    /// A type's states: New and Active are open, Closed and Removed are done.
    pub(crate) fn states() -> Answer {
        Answer::json(&json!({"count": 4, "value": [
            {"name": "New", "category": "Proposed"},
            {"name": "Active", "category": "InProgress"},
            {"name": "Closed", "category": "Completed"},
            {"name": "Removed", "category": "Removed"}
        ]}))
    }
}
