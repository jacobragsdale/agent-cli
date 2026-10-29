//! A spec from a URL (found as `find` finds it) or a file, and the refs
//! `SPEC#OPERATION` and `SPEC#Schema` split.

use agent_cli_core::{Ctx, Failure};
use anyhow::{Context, Result};
use serde_json::Value;

use super::{Operation, Spec, find, template, yaml};
use crate::call::Prepared;

/// The most a spec file may hold.
const FILE_MAX: u64 = 20 * 1024 * 1024;

/// The spec `given` names: a URL (the spec, its docs page or the API's base
/// URL) or a file's path.
pub(crate) fn spec(ctx: &Ctx, prepared: &Prepared<'_>, given: &str) -> Result<Spec> {
    let lower = given.trim().to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let (found, text) = find::find(ctx, prepared, given.trim())?;
        let raw = parse(&text).with_context(|| format!("the spec at {}", found.url))?;
        let mut spec = Spec::read(&found.url, raw)?;
        spec.found = Some(found);
        return Ok(spec);
    }
    let meta = std::fs::metadata(given).map_err(|why| {
        Failure::usage(format!("{given}: {why}")).hint(
            "give the spec's URL, its docs page, the API's base URL (https://…), or a spec file",
        )
    })?;
    if meta.len() > FILE_MAX {
        return Err(Failure::usage(format!("{given} is over 20 MB, more than any spec")).into());
    }
    let text = std::fs::read_to_string(given).with_context(|| format!("reading {given}"))?;
    Spec::read(
        given,
        parse(&text).with_context(|| format!("the spec in {given}"))?,
    )
}

/// A spec's text as JSON, whichever it was written in.
pub(crate) fn parse(text: &str) -> Result<Value> {
    if text.trim_start().starts_with('{') {
        return Ok(serde_json::from_str(text)?);
    }
    yaml::parse(text)
}

/// `SPEC#REST` as the spec and the rest; with `--spec`, `REST` alone works,
/// and a ref naming another spec is exit 2.
pub(crate) fn split<'a>(
    given: &'a str,
    flag: Option<&'a str>,
    what: &str,
) -> Result<(&'a str, &'a str)> {
    match (given.split_once('#'), flag) {
        (Some((spec, _)), Some(flag)) if spec != flag => Err(Failure::usage(format!(
            "{given:?} names the spec {spec}, but --spec says {flag}"
        ))
        .into()),
        (Some((spec, rest)), _) => Ok((spec, rest)),
        (None, Some(flag)) => Ok((flag, given)),
        (None, None) => Err(Failure::usage(format!(
            "{given:?} names no spec: write SPEC#{what} or give --spec SPEC"
        ))
        .hint("the ids `agent-cli http operation list SPEC` prints carry it")
        .into()),
    }
}

/// The operation `wanted` names: `METHOD PATH` (its parameters' names may
/// differ) or an operationId.
pub(crate) fn operation<'s>(spec: &'s Spec, wanted: &str) -> Result<&'s Operation> {
    let wanted = wanted.trim();
    if let Some((method, path)) = wanted.split_once(' ') {
        let (method, path) = (method.to_ascii_uppercase(), template(path.trim()));
        if let Some(found) = spec
            .operations
            .iter()
            .find(|op| op.method == method && template(&op.path) == path)
        {
            return Ok(found);
        }
    }
    let by_id = |exact: bool| {
        spec.operations.iter().find(|op| {
            op.operation_id.as_deref().is_some_and(|id| {
                if exact {
                    id == wanted
                } else {
                    id.eq_ignore_ascii_case(wanted)
                }
            })
        })
    };
    if let Some(found) = by_id(true).or_else(|| by_id(false)) {
        return Ok(found);
    }
    // Its neighbours: the same path by another method first, then paths
    // sharing its last literal segment.
    let path = wanted
        .split_once(' ')
        .map_or(wanted, |(_, path)| path.trim());
    let last = path
        .rsplit('/')
        .find(|part| !part.is_empty() && !part.starts_with('{'))
        .unwrap_or(path);
    let mut near: Vec<&Operation> = spec
        .operations
        .iter()
        .filter(|op| {
            op.path.contains(last)
                || op
                    .operation_id
                    .as_deref()
                    .is_some_and(|id| id.contains(last))
        })
        .collect();
    near.sort_by_key(|op| template(&op.path) != template(path));
    let near: Vec<String> = near
        .iter()
        .take(5)
        .map(|op| format!("{} {}", op.method, op.path))
        .collect();
    let near = if near.is_empty() {
        String::new()
    } else {
        format!("; near it: {}", near.join(", "))
    };
    Err(
        Failure::not_found(format!("{} has no operation {wanted:?}{near}", spec.id))
            .hint(format!(
                "agent-cli http operation list {}  lists them",
                spec.id
            ))
            .into(),
    )
}
