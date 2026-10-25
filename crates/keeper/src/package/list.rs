//! `keeper package list`: `GET /packages` (every hosted repository, or one),
//! or for a PyPI proxy the projects it has cached files of.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use super::{is_proxy, proxy_of_pypi};
use crate::client::{Keeper, normalize, note_more, stamp, text};
use crate::ids;

#[derive(clap::Args)]
pub struct PackageListArgs {
    /// Only packages whose name contains this
    name: Option<String>,
    /// Only this repository's (a PyPI proxy lists what it has cached)
    #[arg(long)]
    repo: Option<String>,
    /// Only this format: pypi, generic, npm, maven …
    #[arg(long)]
    format: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A package, by its newest version.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PackageRow {
    /// REPO/PACKAGE: what package get takes.
    id: String,
    repository: String,
    name: String,
    /// The newest version (none for a proxy's cache).
    latest: Option<String>,
    format: Option<String>,
    /// Bytes of the newest version.
    size: Option<u64>,
    downloads: Option<u64>,
    updated: Option<String>,
    /// A proxy: how many of its files are cached.
    cached_files: Option<usize>,
}

/// The most cached files read to list what a proxy holds.
// ponytail: a proxy with more cached files than this lists the projects
// of the first ones by path, and says so.
const PROXY_FILES_MAX: usize = 2000;

fn package_list(ctx: &Ctx, args: PackageListArgs) -> Result<Vec<PackageRow>> {
    let keeper = Keeper::load(ctx)?;
    let repo = args
        .repo
        .as_deref()
        .map(|raw| ids::repo(&keeper, raw))
        .transpose()?;
    if let Some(key) = &repo {
        let found = keeper.repo(ctx, key)?;
        if is_proxy(&found) {
            proxy_of_pypi(&found, key)?;
            return cached(ctx, &keeper, key, args.name.as_deref(), args.limit);
        }
    }
    let mut query = Vec::new();
    if let Some(key) = &repo {
        query.push(("repository_key", key.clone()));
    }
    if let Some(name) = args.name {
        query.push(("search", name));
    }
    if let Some(format) = args.format {
        query.push(("format", format.to_ascii_lowercase()));
    }
    let (found, total) = keeper.pages(ctx, "/packages", &query, args.limit)?;
    let mut rows: Vec<PackageRow> = found
        .iter()
        .filter_map(|package| {
            let repository = text(&package["repository_key"])?;
            let name = text(&package["name"])?;
            Some(PackageRow {
                id: format!("{repository}/{name}"),
                latest: text(&package["version"]),
                format: text(&package["format"]),
                size: package["size_bytes"].as_u64(),
                downloads: package["download_count"].as_u64(),
                updated: stamp(&package["updated_at"]),
                cached_files: None,
                repository,
                name,
            })
        })
        .collect();
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        note_more(ctx, args.limit, total);
    }
    if rows.is_empty() && repo.is_none() {
        ctx.note("[none hosted; a PyPI proxy's cache lists with --repo PROXY: agent-cli keeper repo list --type remote]");
    }
    Ok(rows)
}

/// A PyPI proxy's cached files (`simple/PROJECT/FILE`) by project.
fn cached(
    ctx: &Ctx,
    keeper: &Keeper,
    key: &str,
    name: Option<&str>,
    limit: usize,
) -> Result<Vec<PackageRow>> {
    let files = keeper.artifacts(
        ctx,
        key,
        &[("path_prefix", "simple/".to_owned())],
        PROXY_FILES_MAX,
    )?;
    if files.len() > PROXY_FILES_MAX {
        ctx.note(format!(
            "[the projects of the first {PROXY_FILES_MAX} cached files by path]"
        ));
    }
    let wanted = name.map(normalize);
    let mut projects: BTreeMap<String, (usize, Option<String>)> = BTreeMap::new();
    for file in files.iter().take(PROXY_FILES_MAX) {
        let Some(path) = text(&file["path"]) else {
            continue;
        };
        let mut parts = path.split('/').skip(1);
        let (Some(project), Some(_)) = (parts.next(), parts.next()) else {
            continue;
        };
        if wanted
            .as_ref()
            .is_some_and(|wanted| !normalize(project).contains(wanted))
        {
            continue;
        }
        let entry = projects.entry(project.to_owned()).or_default();
        entry.0 += 1;
        entry.1 = entry.1.clone().max(stamp(&file["cache_cached_at"]));
    }
    let shown = projects.len();
    let mut rows: Vec<PackageRow> = projects
        .into_iter()
        .map(|(project, (count, updated))| PackageRow {
            id: format!("{key}/{project}"),
            repository: key.to_owned(),
            name: project,
            latest: None,
            format: Some("pypi".to_owned()),
            size: None,
            downloads: None,
            updated,
            cached_files: Some(count),
        })
        .collect();
    if rows.len() > limit {
        rows.truncate(limit);
        note_more(ctx, limit, Some(shown as u64));
    }
    Ok(rows)
}

command! {
    pub PACKAGE_LIST = ["keeper", "package", "list"], Read,
    "List packages in Artifact Keeper by name, or what a PyPI proxy has cached",
    keywords: ["pypi", "pip", "python", "internal", "libraries", "cached", "proxy", "search", "find"],
    example: "keeper package list contoso --fields id,latest,updated",
    run: package_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{API, artifact, files, keeper_run, page, repository, urls};

    #[test]
    fn packages_list_by_name_across_hosted_repositories() {
        let package = json!({"id": "00000000-0000-4000-8000-00000000c001", "repository_key": "pypi",
            "name": "contoso-orders-client", "version": "2.3.0", "format": "pypi", "description": null,
            "size_bytes": 4096, "download_count": 41, "created_at": "2026-08-01T10:00:00Z",
            "updated_at": "2026-09-28T20:05:00.4Z", "metadata": {}});
        let (outcome, transport) = keeper_run(
            &["keeper", "package", "list", "orders", "--format", "PyPI"],
            vec![page(vec![package], 1)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport),
            [format!(
                "{API}/packages?search=orders&format=pypi&page=1&per_page=51"
            )]
        );
        assert_eq!(
            outcome.json(),
            json!([{"id": "pypi/contoso-orders-client", "repository": "pypi", "name": "contoso-orders-client",
                "latest": "2.3.0", "format": "pypi", "size": 4096, "downloads": 41, "updated": "2026-09-28T20:05:00Z"}])
        );
    }

    #[test]
    fn a_proxy_lists_the_projects_it_has_cached_files_of() {
        let cached = |path: &str| {
            let mut file = artifact("pypi-proxy", path, "ee");
            file["cache_cached_at"] = json!("2026-09-20T08:00:00Z");
            file
        };
        let (outcome, transport) = keeper_run(
            &["keeper", "package", "list", "--repo", "pypi-proxy"],
            vec![
                Answer::json(&repository("pypi-proxy", "pypi", "remote")),
                files(
                    vec![
                        cached("simple/requests/requests-2.32.3-py3-none-any.whl"),
                        cached("simple/requests/requests-2.32.3.tar.gz"),
                        cached("simple/six/six-1.17.0-py2.py3-none-any.whl"),
                    ],
                    None,
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport)[1],
            format!("{API}/repositories/pypi-proxy/artifacts?path_prefix=simple%2F&per_page=100")
        );
        assert_eq!(
            outcome.json(),
            json!([{"id": "pypi-proxy/requests", "repository": "pypi-proxy", "name": "requests",
                    "format": "pypi", "updated": "2026-09-20T08:00:00Z", "cached_files": 2},
                   {"id": "pypi-proxy/six", "repository": "pypi-proxy", "name": "six",
                    "format": "pypi", "updated": "2026-09-20T08:00:00Z", "cached_files": 1}])
        );
    }
}
