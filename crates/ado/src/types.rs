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

/// Always required, yet Azure DevOps fills them from the area and iteration
/// paths, which default to the project's root.
const SYSTEM_SET: [&str; 2] = ["System.AreaId", "System.IterationId"];

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
    /// The caller must set it: always required, with no default, and not
    /// one the system sets.
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
    Ok(read_states(ctx, ado, kind, false)?.0)
}

/// The states, and whether they came from the cache (`fresh` skips it).
fn read_states(ctx: &Ctx, ado: &Ado, kind: &str, fresh: bool) -> Result<(Vec<State>, bool)> {
    let key = cache_key(ado, "states", kind);
    if !fresh && let Some(states) = ctx.cache().get(&key) {
        return Ok((states, true));
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
    Ok((states, false))
}

/// Whether `state` is finished work for `kind`: its category is Completed or
/// Removed.
pub(crate) fn done(ctx: &Ctx, ado: &Ado, kind: &str, state: &str) -> Result<bool> {
    let (mut states, cached) = read_states(ctx, ado, kind, false)?;
    // A state the cached list lacks may be newer than the cache: read again.
    if cached
        && !states
            .iter()
            .any(|s| s.name.eq_ignore_ascii_case(state.trim()))
    {
        states = read_states(ctx, ado, kind, true)?.0;
    }
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
    Ok(read_fields(ctx, ado, kind, false)?.0)
}

/// The type's fields, read again when the cached list lacks one of `names`
/// (by reference or display name): it may be newer than the cache.
pub(crate) fn fields_naming(
    ctx: &Ctx,
    ado: &Ado,
    kind: &str,
    names: &[&str],
) -> Result<Vec<Field>> {
    let (fields, cached) = read_fields(ctx, ado, kind, false)?;
    let named = |name: &&str| {
        (fields.iter())
            .any(|f| f.reference.eq_ignore_ascii_case(name) || f.name.eq_ignore_ascii_case(name))
    };
    if cached && !names.iter().all(named) {
        return Ok(read_fields(ctx, ado, kind, true)?.0);
    }
    Ok(fields)
}

/// The fields, and whether they came from the cache (`fresh` skips it).
fn read_fields(ctx: &Ctx, ado: &Ado, kind: &str, fresh: bool) -> Result<(Vec<Field>, bool)> {
    let key = cache_key(ado, "fields", kind);
    if !fresh && let Some(fields) = ctx.cache().get(&key) {
        return Ok((fields, true));
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
            let default = plain(&field["defaultValue"]);
            Some(Field {
                name: text(&field["name"]).unwrap_or_else(|| reference.clone()),
                kind: types.get(reference.as_str()).map(|kind| (*kind).to_owned()),
                required: field["alwaysRequired"].as_bool().unwrap_or(false)
                    && default.is_none()
                    && !SYSTEM_SET.contains(&reference.as_str()),
                allowed_values: list(&field["allowedValues"])
                    .iter()
                    .filter_map(plain)
                    .collect(),
                default,
                reference,
            })
        })
        .collect();
    ctx.cache().put(&key, &fields, CACHE_TTL);
    Ok((fields, false))
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
    fn done_goes_by_category_and_the_states_are_read_once_a_day_or_on_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let mut states = vec![
            json!({"name": "New", "color": "b2b2b2", "category": "Proposed"}),
            json!({"name": "Active", "color": "007acc", "category": "InProgress"}),
            json!({"name": "Shipped", "color": "339933", "category": "Completed"}),
            json!({"name": "Cut", "color": "ffffff", "category": "Removed"}),
        ];
        let before = Answer::json(&json!({"value": states}));
        states.push(json!({"name": "Closed", "category": "Completed"}));
        let after = Answer::json(&json!({"value": states}));
        let transport = FakeTransport::answering([before, after.clone(), after]);
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
        assert_eq!(transport.sent().len(), 1);
        // A state added since the list was cached is read again, not refused.
        assert!(done("Closed"));
        let error = super::done(&ctx, &ado, "User Story", "Nope").unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.exit, Exit::Usage);
        assert_eq!(
            failure.hint.as_deref(),
            Some("agent-cli ado workitem-type get \"User Story\"")
        );
        assert_eq!(transport.sent().len(), 3);
        assert_eq!(
            transport.sent()[0].url,
            "https://dev.azure.com/contoso/Fabrikam/_apis/wit/workitemtypes/User%20Story/states?api-version=7.1"
        );
    }

    #[test]
    fn a_field_the_cached_list_lacks_reads_the_fields_again() {
        let dir = tempfile::tempdir().unwrap();
        let types = || {
            crate::testing::page(vec![
                json!({"referenceName": "System.Title", "type": "string"}),
            ])
        };
        let title = json!({"name": "Title", "referenceName": "System.Title"});
        let effort = json!({"name": "Effort", "referenceName": "Microsoft.VSTS.Scheduling.Effort"});
        let transport = FakeTransport::answering([
            crate::testing::page(vec![title.clone()]),
            types(),
            crate::testing::page(vec![title, effort]),
            types(),
        ]);
        let ctx = ctx(Setup {
            cache_dir: Some(dir.path().to_owned()),
            ..Setup::fake(transport.clone())
        });
        let config = "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\n";
        let ado = Ado::names(&Config::parse("c.toml", Some(config), Vec::new())).unwrap();
        let names = |names: &[&str]| -> Vec<String> {
            let fields = fields_naming(&ctx, &ado, "Task", names).unwrap();
            fields.into_iter().map(|field| field.name).collect()
        };
        // Read fresh, the list is what the type has: no second read.
        assert_eq!(names(&["Effort"]), ["Title"]);
        assert_eq!(transport.sent().len(), 2);
        assert_eq!(names(&["title"]), ["Title"]);
        assert_eq!(transport.sent().len(), 2);
        assert_eq!(names(&["effort"]), ["Title", "Effort"]);
        assert_eq!(transport.sent().len(), 4);
    }
}
