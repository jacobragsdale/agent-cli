//! What the page verbs share: the row a write answers with, the people a
//! body mentions, and the long-text limits.

pub(crate) mod comment;
pub(crate) mod create;
pub(crate) mod delete;
pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod update;

use std::collections::HashMap;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Confluence, text};
use crate::compose::mentioned;

/// v2 refuses a request body over 5 MB.
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

/// The account id of each `@<Name>` in `markdown`: the people the page
/// already mentions (`known`, by id) first, then a user search; an account
/// id written as the name stands for itself. Nobody by that name is exit
/// 4; two people is exit 2.
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
        .map(|(id, name)| (name.to_lowercase(), id))
        .collect();
    for name in names {
        if name.contains(':') || (name.len() >= 24 && name.chars().all(|c| c.is_ascii_hexdigit())) {
            found.insert(name.clone(), name);
            continue;
        }
        if let Some(id) = on_page.get(&name.to_lowercase()) {
            found.insert(name, id.clone());
            continue;
        }
        let cql = format!(
            "user.fullname ~ \"{}\"",
            name.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let url = confluence.v1("/search/user", &[("cql", cql), ("limit", "10".to_owned())]);
        let answer = confluence.get(ctx, &url)?;
        let matches: Vec<String> = answer["results"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| {
                text(&row["user"]["displayName"])
                    .or_else(|| text(&row["user"]["publicName"]))
                    .is_some_and(|shown| shown.eq_ignore_ascii_case(&name))
            })
            .filter_map(|row| text(&row["user"]["accountId"]))
            .collect();
        match matches[..] {
            [ref id] => {
                found.insert(name, id.clone());
            }
            [] => {
                return Err(
                    Failure::not_found(format!("no one is named {name:?} on the site"))
                        .hint("write the name as plain text, or @<ACCOUNT-ID>")
                        .into(),
                );
            }
            _ => {
                return Err(Failure::usage(format!(
                    "{} people are named {name:?}: {}",
                    matches.len(),
                    matches.join(", ")
                ))
                .hint("write @<ACCOUNT-ID> for the one you mean")
                .into());
            }
        }
    }
    Ok(found)
}

/// A comma-separated label list, trimmed, empties dropped.
pub(crate) fn labels(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
        .collect()
}
