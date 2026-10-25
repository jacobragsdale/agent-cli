//! What the page verbs share: the row a write answers with, the people a
//! body mentions, and the long-text limits.

pub(crate) mod comment;
pub(crate) mod create;
pub(crate) mod delete;
pub(crate) mod diff;
pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod update;

use std::collections::HashMap;

use agent_cli_core::{Ctx, Failure, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Confluence, person, text};
use crate::compose::mentioned;

/// The most a body may be; past it, a page is no longer one to edit as text.
pub(crate) const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// A page after a write.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Written {
    /// What page get takes.
    pub(crate) id: String,
    pub(crate) title: Option<String>,
    pub(crate) space: Option<String>,
    pub(crate) version: Option<u64>,
    pub(crate) url: Option<String>,
}

/// The user key of each `@<Name>` in `markdown`: the people the page
/// already mentions (`known`, by key) first, then a search by full name,
/// then the name as a username. Nobody by that name is exit 4; two people
/// is exit 2.
pub(crate) fn people(
    ctx: &Ctx,
    confluence: &Confluence,
    markdown: &str,
    known: &[String],
) -> Result<HashMap<String, String>> {
    let names = mentioned(markdown);
    let mut found = HashMap::new();
    if names.is_empty() {
        return Ok(found);
    }
    let on_page: HashMap<String, String> = confluence
        .names(ctx, known)
        .into_iter()
        .map(|(key, name)| (name.to_lowercase(), key))
        .collect();
    for name in names {
        if let Some(key) = on_page.get(&name.to_lowercase()) {
            found.insert(name, key.clone());
            continue;
        }
        let cql = format!(
            "type = user and user.fullname ~ \"{}\"",
            name.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let url = confluence.api("/search", &[("cql", cql), ("limit", "10".to_owned())]);
        let answer = confluence.get(ctx, &url)?;
        let matches: Vec<(String, String)> = answer["results"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| &row["user"])
            .filter(|user| person(user).is_some_and(|shown| shown.eq_ignore_ascii_case(&name)))
            .filter_map(|user| {
                Some((
                    text(&user["userKey"])?,
                    text(&user["username"]).unwrap_or_default(),
                ))
            })
            .collect();
        match &matches[..] {
            [(key, _)] => {
                found.insert(name, key.clone());
            }
            [] => {
                let key = username(ctx, confluence, &name)?.ok_or_else(|| {
                    Failure::not_found(format!("no one is named {name:?} on the site"))
                        .hint("write the name as plain text, or @<USERNAME>")
                })?;
                found.insert(name, key);
            }
            _ => {
                let usernames: Vec<&str> = matches.iter().map(|(_, user)| user.as_str()).collect();
                return Err(Failure::usage(format!(
                    "{} people are named {name:?}: {}",
                    matches.len(),
                    usernames.join(", ")
                ))
                .hint("write @<USERNAME> for the one you mean")
                .into());
            }
        }
    }
    Ok(found)
}

/// The user key of the user `name` is the username of, if any.
fn username(ctx: &Ctx, confluence: &Confluence, name: &str) -> Result<Option<String>> {
    if name.chars().any(char::is_whitespace) {
        return Ok(None);
    }
    let url = confluence.api("/user", &[("username", name.to_owned())]);
    match confluence.get(ctx, &url) {
        Ok(user) => Ok(text(&user["userKey"])),
        Err(error) if status_of(&error) == Some(404) => Ok(None),
        Err(error) => Err(error),
    }
}

/// A comma-separated label list, trimmed, empties dropped.
pub(crate) fn labels(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
        .collect()
}
