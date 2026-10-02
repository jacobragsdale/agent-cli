//! A work item type's rules: its states (and which of them count as done)
//! and its fields, each cached for a day since a process changes about never.
//! Nothing guesses "done" from a state's name: a team's `Shipped` is as done
//! as `Closed` when its category says so.

use std::collections::HashMap;
use std::time::Duration;

use agent_cli_core::{Ctx, Failure, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Ado, list, segment, text};

const CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub(crate) struct State {
    pub(crate) name: String,
    /// Proposed, InProgress, Resolved, Completed or Removed.
    pub(crate) category: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Field {
    pub(crate) name: String,
    /// The reference name, `Microsoft.VSTS.Scheduling.StoryPoints`.
    #[serde(rename = "ref")]
    pub(crate) reference: String,
    /// string, integer, double, html, identity, dateTime, treePath, boolean …
    #[serde(rename = "type")]
    pub(crate) kind: Option<String>,
    pub(crate) required: bool,
    pub(crate) allowed_values: Vec<String>,
    pub(crate) default: Option<String>,
}

/// `{project}/_apis/wit/workitemtypes/{kind}{path}`, where a 404 means the
/// type is not in the project.
pub(crate) fn read_type(
    ctx: &Ctx,
    ado: &Ado,
    kind: &str,
    path: &str,
    query: &str,
) -> Result<Value> {
    let url = ado.work(
        &format!("wit/workitemtypes/{}{path}", segment(kind.trim())),
        query,
    );
    ado.get(ctx, &url).map_err(|error| {
        if status_of(&error) == Some(404) {
            Failure::not_found(format!("{} has no work item type {kind:?}", ado.project))
                .hint("agent-cli ado workitem-type list")
                .into()
        } else {
            error
        }
    })
}

fn cache_key(ado: &Ado, what: &str, kind: &str) -> String {
    ado.cache_key(&format!(
        "{what}:{}:{}",
        ado.project.to_lowercase(),
        kind.trim().to_lowercase()
    ))
}

/// The type's states in workflow order.
pub(crate) fn states(ctx: &Ctx, ado: &Ado, kind: &str) -> Result<Vec<State>> {
    let key = cache_key(ado, "states", kind);
    if let Some(states) = ctx.cache().get(&key) {
        return Ok(states);
    }
    let answer = read_type(ctx, ado, kind, "/states", "")?;
    let states: Vec<State> = list(&answer["value"])
        .iter()
        .filter_map(|state| {
            Some(State {
                name: text(&state["name"])?,
                category: text(&state["category"]),
            })
        })
        .collect();
    ctx.cache().put(&key, &states, CACHE_TTL);
    Ok(states)
}

/// Whether `state` is finished work for `kind`: its category is Completed or
/// Removed.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "sprint totals, the rollover and tree roll-ups are its callers to come"
    )
)]
pub(crate) fn done(ctx: &Ctx, ado: &Ado, kind: &str, state: &str) -> Result<bool> {
    let states = states(ctx, ado, kind)?;
    let Some(found) = states
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(state.trim()))
    else {
        let names: Vec<&str> = states.iter().map(|s| s.name.as_str()).collect();
        return Err(Failure::usage(format!(
            "{kind} has no state {state:?}; its states are: {}",
            names.join(", ")
        ))
        .hint(format!("agent-cli ado workitem-type get {kind:?}"))
        .into());
    };
    Ok(matches!(
        found.category.as_deref(),
        Some("Completed" | "Removed")
    ))
}

/// The type's fields with their rules, and each field's data type from the
/// project's field list (the type's own list does not say it).
pub(crate) fn fields(ctx: &Ctx, ado: &Ado, kind: &str) -> Result<Vec<Field>> {
    let key = cache_key(ado, "fields", kind);
    if let Some(fields) = ctx.cache().get(&key) {
        return Ok(fields);
    }
    let answer = read_type(ctx, ado, kind, "/fields", "$expand=allowedValues")?;
    let project = ado.get(ctx, &ado.work("wit/fields", ""))?;
    let types: HashMap<&str, &str> = list(&project["value"])
        .iter()
        .filter_map(|field| Some((field["referenceName"].as_str()?, field["type"].as_str()?)))
        .collect();
    let fields: Vec<Field> = list(&answer["value"])
        .iter()
        .filter_map(|field| {
            let reference = text(&field["referenceName"])?;
            Some(Field {
                name: text(&field["name"]).unwrap_or_else(|| reference.clone()),
                kind: types.get(reference.as_str()).map(|kind| (*kind).to_owned()),
                required: field["alwaysRequired"].as_bool().unwrap_or(false),
                allowed_values: list(&field["allowedValues"])
                    .iter()
                    .filter_map(plain)
                    .collect(),
                default: plain(&field["defaultValue"]),
                reference,
            })
        })
        .collect();
    ctx.cache().put(&key, &fields, CACHE_TTL);
    Ok(fields)
}

/// A value as text: allowed values and defaults come as strings or numbers.
fn plain(value: &Value) -> Option<String> {
    match value {
        Value::String(_) => text(value),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, FakeTransport, ctx};
    use agent_cli_core::{Config, Exit, Setup};
    use serde_json::json;

    use super::*;

    #[test]
    fn done_goes_by_category_and_the_states_are_read_once_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let transport = FakeTransport::answering([Answer::json(&json!({"count": 4, "value": [
            {"name": "New", "color": "b2b2b2", "category": "Proposed"},
            {"name": "Active", "color": "007acc", "category": "InProgress"},
            {"name": "Shipped", "color": "339933", "category": "Completed"},
            {"name": "Cut", "color": "ffffff", "category": "Removed"}
        ]}))]);
        let setup = Setup {
            cache_dir: Some(dir.path().to_owned()),
            ..Setup::fake(transport.clone())
        };
        let ctx = ctx(setup);
        let config = "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\n";
        let ado = Ado::names(&Config::parse("c.toml", Some(config), Vec::new())).unwrap();
        let done = |state: &str| done(&ctx, &ado, "User Story", state).unwrap();
        assert!(done("Shipped") && done("cut"));
        assert!(!done("New") && !done("Active"));
        let error = super::done(&ctx, &ado, "User Story", "Closed").unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.exit, Exit::Usage);
        assert_eq!(
            failure.hint.as_deref(),
            Some("agent-cli ado workitem-type get \"User Story\"")
        );
        assert_eq!(transport.sent().len(), 1);
        assert_eq!(
            transport.sent()[0].url,
            "https://dev.azure.com/contoso/Fabrikam/_apis/wit/workitemtypes/User%20Story/states?api-version=7.1"
        );
    }
}
