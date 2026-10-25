//! Packages: a hosted one is a row of `/packages` (Keeper reads its name
//! and version from the upload: PyPI metadata, or a generic path's first two
//! segments, `NAME/VERSION/FILE`); a PyPI proxy holds none, so its packages
//! are upstream's, read through the proxy's PyPI JSON API, and its cache is
//! the files under `simple/NAME/`.

pub(crate) mod get;
pub(crate) mod list;

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Exit, Failure, status_of};
use anyhow::Result;
use serde_json::Value;

use crate::client::{Keeper, list, normalize, segment, text};

/// The most cached files read to say what a proxy holds of one package.
// ponytail: a package with more cached files than this reads as partly
// cached; no PyPI project the team uses comes near it.
const CACHED_MAX: usize = 1000;

/// A hosted (or virtual) repository's package by name: the `/packages`
/// search, then the row whose name is `name`, exactly or as PEP 503
/// compares them. Keeper's search matches the stored spelling, so a name
/// it finds nothing for is searched again in PEP 503 form.
pub(crate) fn hosted(ctx: &Ctx, keeper: &Keeper, repo: &str, name: &str) -> Result<Value> {
    let named = |row: &&Value| text(&row["name"]).as_deref() == Some(name);
    let alike =
        |row: &&Value| text(&row["name"]).is_some_and(|it| normalize(&it) == normalize(name));
    let mut searches = vec![name.to_owned()];
    if normalize(name) != name {
        searches.push(normalize(name));
    }
    for search in searches {
        let query = [
            ("repository_key", repo.to_owned()),
            ("search", search),
            ("per_page", "100".to_owned()),
        ];
        let found = keeper.get(ctx, &keeper.api("/packages", &query))?;
        let rows = list(&found["items"]);
        if let Some(row) = rows.iter().find(named).or_else(|| rows.iter().find(alike)) {
            return Ok(row.clone());
        }
    }
    Err(Failure::not_found(format!("no package {name} in {repo}"))
        .hint(format!("agent-cli keeper package list {name}"))
        .into())
}

/// What a PyPI proxy's upstream says of `name` (or of one release), asked
/// through the proxy as pip would be: a 404 is upstream not having it, a
/// 502 the proxy not reaching upstream.
pub(crate) fn upstream(
    ctx: &Ctx,
    keeper: &Keeper,
    repo: &Value,
    name: &str,
    version: Option<&str>,
) -> Result<Value> {
    let key = text(&repo["key"]).unwrap_or_default();
    let origin = text(&repo["upstream_url"]).unwrap_or_else(|| "its upstream".to_owned());
    let release = version
        .map(|version| format!("/{}", segment(version)))
        .unwrap_or_default();
    let url = format!(
        "{}/pypi/{}/pypi/{}{release}/json",
        keeper.base,
        segment(&key),
        segment(name)
    );
    let what = match version {
        Some(version) => format!("{name} {version}"),
        None => name.to_owned(),
    };
    match keeper.get(ctx, &url) {
        Err(error) if status_of(&error) == Some(404) => Err(Failure::not_found(format!(
            "{origin} has no {what} (asked through the proxy {key}), so pip cannot install it through it"
        ))
        .hint(match version {
            Some(_) => format!("agent-cli keeper package get {key}/{name}"),
            None => format!("agent-cli keeper package list {name}"),
        })
        .into()),
        Err(error) if matches!(status_of(&error), Some(502 | 504)) => Err(Failure::new(
            Exit::Failed,
            format!("the proxy {key} could not reach {origin} for {what}: {error:#}"),
        )
        .hint(format!("agent-cli keeper repo get {key}"))
        .into()),
        found => found,
    }
}

/// The files a PyPI proxy has cached of `name`: file name to when.
pub(crate) fn cached(
    ctx: &Ctx,
    keeper: &Keeper,
    repo: &str,
    name: &str,
) -> Result<BTreeMap<String, Option<String>>> {
    let prefix = format!("simple/{}/", normalize(name));
    let files = keeper.artifacts(ctx, repo, &[("path_prefix", prefix)], CACHED_MAX)?;
    Ok(files
        .iter()
        .filter_map(|file| {
            let path = text(&file["path"])?;
            let name = path.rsplit('/').next()?.to_owned();
            Some((name, crate::client::stamp(&file["cache_cached_at"])))
        })
        .collect())
}

/// Whether a repository answers through upstream rather than from its own
/// uploads: a PyPI proxy.
pub(crate) fn is_proxy(repo: &Value) -> bool {
    repo["repo_type"] == "remote"
}

/// A proxy of anything but PyPI is read as files only.
pub(crate) fn proxy_of_pypi(repo: &Value, key: &str) -> Result<()> {
    if repo["format"] == "pypi" {
        return Ok(());
    }
    Err(Failure::usage(format!(
        "{key} is a {} proxy; packages are read through PyPI proxies and hosted repositories",
        text(&repo["format"]).unwrap_or_default()
    ))
    .hint(format!("agent-cli keeper file list {key}"))
    .into())
}
