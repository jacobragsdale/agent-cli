//! Files in a repository at a ref. `fetch` is also how `thread list` and
//! `diff get` read the code they show.

pub(crate) mod get;
pub(crate) mod list;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use serde_json::Value;

use crate::client::{API, Ado, query_value};
use crate::ids::{items_path, version_query};

/// One file's item, content included, at `reading` (a `versionType` and
/// version; `None` is the default branch).
pub(crate) fn fetch(
    ctx: &Ctx,
    ado: &Ado,
    project: &str,
    repo: &str,
    path: &str,
    reading: Option<&(&str, String)>,
) -> Result<Value> {
    let query = format!(
        "path={}&{}includeContent=true&$format=json",
        query_value(&format!("/{}", path.trim_start_matches('/'))),
        version_query(reading)
    );
    ado.get(ctx, &ado.api(Some(project), &items_path(repo), &query, API))
}

/// A fetched item's text, refusing a folder or a binary file; `id` names it.
pub(crate) fn text_of(item: &Value, id: &str) -> Result<String> {
    if item["isFolder"].as_bool() == Some(true) {
        return Err(Failure::usage(format!("{id} is a folder"))
            .hint(format!("agent-cli ado file list {id}"))
            .into());
    }
    let content = item["content"].as_str().unwrap_or_default();
    // ponytail: a NUL byte is how a binary file shows; contentMetadata.isBinary
    // needs includeContentMetadata, one more parameter on every read.
    if content.contains('\0') {
        return Err(Failure::usage(format!(
            "{id} is a binary file ({} bytes); file get shows text",
            content.len()
        ))
        .into());
    }
    Ok(content.to_owned())
}

/// Lines `first..=last` of `lines` (counted from 1), each after its number.
pub(crate) fn numbered(lines: &[&str], first: usize, last: usize) -> String {
    let width = last.to_string().len();
    (first..=last.min(lines.len()))
        .map(|number| format!("{number:>width$}  {}", lines[number - 1]))
        .collect::<Vec<_>>()
        .join("\n")
}
