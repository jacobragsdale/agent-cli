//! `keeper package get`: a package and its versions, newest first. Hosted:
//! `/packages` found by name, then `/packages/{id}/versions`. A PyPI proxy:
//! upstream's releases through the proxy's PyPI JSON API, each marked with
//! whether the proxy has cached a file of it.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::{cached, hosted, is_proxy, proxy_of_pypi, upstream};
use crate::client::{Keeper, list, moment, segment, stamp, text};
use crate::ids;

#[derive(clap::Args)]
pub struct PackageGetArgs {
    /// The package: REPO/PACKAGE, as package list prints it, PACKAGE with --repo, or its web or pip index URL
    package: String,
    /// The repository, when PACKAGE is a bare name
    #[arg(long)]
    repo: Option<String>,
    /// Most versions to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A package and what versions of it there are.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PackageDetail {
    /// REPO/PACKAGE: what package get takes.
    id: String,
    repository: String,
    name: String,
    format: Option<String>,
    /// local, virtual, or remote (a proxy: its versions are upstream's).
    #[serde(rename = "type")]
    kind: Option<String>,
    /// The newest version.
    latest: Option<String>,
    description: Option<String>,
    /// What a proxy asks for what it does not hold.
    upstream: Option<String>,
    /// A proxy: whether it holds any file of the package.
    cached: Option<bool>,
    /// Downloads of every version.
    downloads: Option<u64>,
    /// Newest first.
    versions: Vec<VersionRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct VersionRow {
    /// REPO/PACKAGE@VERSION: what version get takes.
    id: String,
    version: String,
    /// When it was published (upstream's upload time, for a proxy).
    created: Option<String>,
    /// Bytes.
    size: Option<u64>,
    sha256: Option<String>,
    downloads: Option<u64>,
    /// A proxy: whether it has cached a file of this version.
    cached: Option<bool>,
    /// Every file of it is yanked upstream: pip skips it unless pinned.
    yanked: Option<bool>,
    /// The Python versions it installs on, as its files say.
    requires_python: Option<String>,
}

fn package_get(ctx: &Ctx, args: PackageGetArgs) -> Result<PackageDetail> {
    let keeper = Keeper::load(ctx)?;
    let (key, name) = ids::package(&keeper, ctx, &args.package, args.repo.as_deref())?;
    let repo = keeper.repo(ctx, &key)?;
    let mut detail = if is_proxy(&repo) {
        proxied(ctx, &keeper, &repo, &key, &name)?
    } else {
        local(ctx, &keeper, &repo, &key, &name)?
    };
    if detail.versions.len() > args.limit {
        ctx.note(format!(
            "[{} of {} versions; --limit N]",
            args.limit,
            detail.versions.len()
        ));
        detail.versions.truncate(args.limit);
    }
    Ok(detail)
}

fn local(ctx: &Ctx, keeper: &Keeper, repo: &Value, key: &str, name: &str) -> Result<PackageDetail> {
    let found = hosted(ctx, keeper, key, name)?;
    let name = text(&found["name"]).unwrap_or_else(|| name.to_owned());
    let id = text(&found["id"]).unwrap_or_default();
    let answer = keeper.get(
        ctx,
        &keeper.api(&format!("/packages/{}/versions", segment(&id)), &[]),
    )?;
    let mut listed = list(&answer["versions"]);
    listed.sort_by_key(|version| std::cmp::Reverse(moment(&version["created_at"])));
    let versions: Vec<VersionRow> = listed
        .iter()
        .filter_map(|version| {
            let number = text(&version["version"])?;
            Some(VersionRow {
                id: format!("{key}/{name}@{number}"),
                version: number,
                created: stamp(&version["created_at"]),
                size: version["size_bytes"].as_u64(),
                sha256: text(&version["checksum_sha256"]),
                downloads: version["download_count"].as_u64(),
                cached: None,
                yanked: None,
                requires_python: None,
            })
        })
        .collect();
    Ok(PackageDetail {
        id: format!("{key}/{name}"),
        repository: key.to_owned(),
        latest: text(&found["version"]),
        description: text(&found["description"]),
        format: text(&found["format"]),
        kind: text(&repo["repo_type"]),
        upstream: None,
        cached: None,
        downloads: found["download_count"].as_u64(),
        name,
        versions,
    })
}

fn proxied(
    ctx: &Ctx,
    keeper: &Keeper,
    repo: &Value,
    key: &str,
    name: &str,
) -> Result<PackageDetail> {
    proxy_of_pypi(repo, key)?;
    let project = upstream(ctx, keeper, repo, name, None)?;
    let name = text(&project["info"]["name"]).unwrap_or_else(|| name.to_owned());
    let held = cached(ctx, keeper, key, &name)?;
    let mut versions: Vec<VersionRow> = project["releases"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(number, files)| {
            let files = files.as_array().filter(|files| !files.is_empty())?;
            let created = files
                .iter()
                .filter_map(|file| stamp(&file["upload_time_iso_8601"]))
                .max();
            Some(VersionRow {
                id: format!("{key}/{name}@{number}"),
                version: number.clone(),
                created,
                size: None,
                sha256: None,
                downloads: None,
                cached: Some(files.iter().any(|file| {
                    text(&file["filename"]).is_some_and(|file| held.contains_key(&file))
                })),
                yanked: files
                    .iter()
                    .all(|file| file["yanked"] == true)
                    .then_some(true),
                requires_python: files.iter().find_map(|file| text(&file["requires_python"])),
            })
        })
        .collect();
    versions.sort_by(|a, b| b.created.cmp(&a.created));
    Ok(PackageDetail {
        id: format!("{key}/{name}"),
        repository: key.to_owned(),
        format: text(&repo["format"]),
        kind: text(&repo["repo_type"]),
        latest: text(&project["info"]["version"]),
        description: text(&project["info"]["summary"]),
        upstream: text(&repo["upstream_url"]),
        cached: Some(!held.is_empty()),
        downloads: None,
        name,
        versions,
    })
}

command! {
    pub PACKAGE_GET = ["keeper", "package", "get"], Read,
    "Show a package's versions: when each was published, or whether a proxy cached it",
    keywords: ["pypi", "pip", "wheel", "releases", "latest", "newest", "exist", "published", "cached", "proxy", "upstream", "install"],
    example: "keeper package get pypi/contoso-orders-client --fields latest,versions",
    run: package_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{API, BASE, artifact, files, keeper_run, page, repository, urls};

    fn package() -> serde_json::Value {
        json!({"id": "00000000-0000-4000-8000-00000000c001", "repository_key": "pypi",
            "name": "contoso-orders-client", "version": "2.3.0", "format": "pypi",
            "description": "Orders API client", "size_bytes": 4096, "download_count": 41,
            "created_at": "2026-08-01T10:00:00Z", "updated_at": "2026-09-28T20:05:00Z",
            "metadata": {"filename": "contoso_orders_client-2.3.0-py3-none-any.whl", "format": "pypi"}})
    }

    #[test]
    fn a_hosted_package_lists_its_versions_newest_first() {
        let versions = json!({"versions": [
            {"version": "2.2.0", "size_bytes": 4000, "download_count": 30,
             "created_at": "2026-08-01T10:00:00.120Z", "checksum_sha256": "aa"},
            {"version": "2.3.0", "size_bytes": 4096, "download_count": 11,
             "created_at": "2026-09-28T20:05:00.5Z", "checksum_sha256": "bb"}]});
        let (outcome, transport) = keeper_run(
            &[
                "keeper",
                "package",
                "get",
                "Contoso_Orders_Client",
                "--repo",
                "pypi",
                "--limit",
                "1",
            ],
            vec![
                Answer::json(&repository("pypi", "pypi", "local")),
                page(vec![package()], 1),
                Answer::json(&versions),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport),
            [
                format!("{API}/repositories/pypi"),
                format!(
                    "{API}/packages?repository_key=pypi&search=Contoso_Orders_Client&per_page=100"
                ),
                format!("{API}/packages/00000000-0000-4000-8000-00000000c001/versions"),
            ]
        );
        assert_eq!(
            outcome.json(),
            json!({"id": "pypi/contoso-orders-client", "repository": "pypi", "name": "contoso-orders-client",
                "format": "pypi", "type": "local", "latest": "2.3.0", "description": "Orders API client",
                "downloads": 41, "versions": [{"id": "pypi/contoso-orders-client@2.3.0", "version": "2.3.0",
                "created": "2026-09-28T20:05:00Z", "size": 4096, "sha256": "bb", "downloads": 11}]})
        );
        assert!(
            outcome.stderr.contains("[1 of 2 versions; --limit N]"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_proxy_package_is_upstreams_with_what_the_proxy_cached() {
        let file = |name: &str, at: &str| {
            json!({"filename": name, "upload_time_iso_8601": at, "yanked": false,
                "requires_python": ">=3.9", "size": 100, "digests": {"sha256": "cc"}})
        };
        let project = json!({"info": {"name": "pyarrow", "version": "17.0.0", "summary": "Python library for Apache Arrow"},
            "releases": {
                "16.1.0": [file("pyarrow-16.1.0-cp311-cp311-manylinux_2_28_x86_64.whl", "2024-05-14T10:00:00.000000Z")],
                "17.0.0": [file("pyarrow-17.0.0-cp311-cp311-manylinux_2_28_x86_64.whl", "2024-07-17T10:00:00.000000Z")],
                "0.1.0": []}});
        let mut held = artifact(
            "pypi-proxy",
            "simple/pyarrow/pyarrow-16.1.0-cp311-cp311-manylinux_2_28_x86_64.whl",
            "dd",
        );
        held["cache_cached_at"] = json!("2026-09-20T08:00:00.1Z");
        let (outcome, transport) = keeper_run(
            &[
                "keeper",
                "package",
                "get",
                &format!("{BASE}/pypi/pypi-proxy/simple/pyarrow/"),
            ],
            vec![
                Answer::json(&repository("pypi-proxy", "pypi", "remote")),
                Answer::json(&project),
                files(vec![held], None),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport)[1..],
            [
                format!("{BASE}/pypi/pypi-proxy/pypi/pyarrow/json"),
                format!(
                    "{API}/repositories/pypi-proxy/artifacts?path_prefix=simple%2Fpyarrow%2F&per_page=100"
                ),
            ]
        );
        let printed = outcome.json();
        assert_eq!(printed["latest"], "17.0.0");
        assert_eq!(printed["upstream"], "https://pypi.org");
        assert_eq!(printed["cached"], true);
        assert_eq!(
            printed["versions"],
            json!([{"id": "pypi-proxy/pyarrow@17.0.0", "version": "17.0.0", "created": "2024-07-17T10:00:00Z",
                    "cached": false, "requires_python": ">=3.9"},
                   {"id": "pypi-proxy/pyarrow@16.1.0", "version": "16.1.0", "created": "2024-05-14T10:00:00Z",
                    "cached": true, "requires_python": ">=3.9"}])
        );
    }

    #[test]
    fn a_package_upstream_lacks_or_cannot_be_reached_says_why_pip_fails() {
        let proxy = || Answer::json(&repository("pypi-proxy", "pypi", "remote"));
        let (outcome, _) = keeper_run(
            &[
                "keeper",
                "package",
                "get",
                "pypi-proxy/contoso-orders-client",
            ],
            vec![
                proxy(),
                Answer::status(404, r#"{"code":"NOT_FOUND","message":"Package not found"}"#),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("https://pypi.org has no contoso-orders-client"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("agent-cli keeper package list contoso-orders-client")
        );
        let (outcome, _) = keeper_run(
            &["keeper", "package", "get", "pypi-proxy/pyarrow"],
            vec![
                proxy(),
                Answer::status(502, "Failed to fetch from upstream"),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("could not reach https://pypi.org"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("agent-cli keeper repo get pypi-proxy")
        );
    }

    #[test]
    fn a_package_needs_a_repository_and_a_version_ref_points_at_version_get() {
        let (outcome, _) = keeper_run(
            &["keeper", "package", "get", "contoso-orders-client"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        let (outcome, _) = keeper_run(
            &[
                "keeper",
                "package",
                "get",
                "pypi/contoso-orders-client@2.3.0",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli keeper version get pypi/contoso-orders-client@2.3.0")
        );
    }
}
