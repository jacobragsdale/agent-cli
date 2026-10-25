//! `keeper version get`: one version's files. Hosted: the files under
//! `PACKAGE/VERSION/` (where Keeper stores PyPI uploads and where a generic
//! path's name and version come from). A PyPI proxy: upstream's release
//! through the proxy's PyPI JSON API, each file marked cached or not.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Keeper, list, moment, normalize, stamp, text};
use crate::file::FileRow;
use crate::ids;
use crate::package::{cached, hosted, is_proxy, proxy_of_pypi, upstream};

#[derive(clap::Args)]
pub struct VersionGetArgs {
    /// The version: REPO/PACKAGE@VERSION, as package get prints it, PACKAGE@VERSION with --repo, or its PyPI JSON URL
    version: String,
    /// The repository, when VERSION names none
    #[arg(long)]
    repo: Option<String>,
}

/// One version: when and by whom it was published, and its files.
#[derive(Debug, Serialize, JsonSchema)]
pub struct VersionDetail {
    /// REPO/PACKAGE@VERSION: what version get takes.
    id: String,
    repository: String,
    package: String,
    version: String,
    /// When its first file was uploaded (upstream's upload time, for a proxy).
    created: Option<String>,
    /// Who uploaded its first file.
    published_by: Option<String>,
    /// Bytes, every file together.
    size: Option<u64>,
    /// The checksum, when it is one file.
    sha256: Option<String>,
    downloads: Option<u64>,
    /// A proxy: whether it has cached any file of it.
    cached: Option<bool>,
    /// Yanked upstream: pip skips it unless pinned.
    yanked: Option<bool>,
    requires_python: Option<String>,
    files: Vec<FileRow>,
}

fn version_get(ctx: &Ctx, args: VersionGetArgs) -> Result<VersionDetail> {
    let keeper = Keeper::load(ctx)?;
    let (key, name, version) = ids::version(&keeper, &args.version, args.repo.as_deref())?;
    let repo = keeper.repo(ctx, &key)?;
    if is_proxy(&repo) {
        proxy_of_pypi(&repo, &key)?;
        return proxied(ctx, &keeper, &repo, &key, &name, &version);
    }
    let mut stored = name.clone();
    let mut files = under(ctx, &keeper, &key, &stored, &version)?;
    if files.is_empty() {
        // The name as uploaded may differ from the one asked (`Contoso_Orders`
        // for `contoso-orders`): the package's own row says.
        stored = text(&hosted(ctx, &keeper, &key, &name)?["name"]).unwrap_or(stored);
        if stored != name {
            files = under(ctx, &keeper, &key, &stored, &version)?;
        }
    }
    if files.is_empty() {
        return Err(
            Failure::not_found(format!("{key} has no version {version} of {stored}"))
                .hint(format!("agent-cli keeper package get {key}/{stored}"))
                .into(),
        );
    }
    files.sort_by_key(|file| moment(&file["created_at"]));
    let rows: Vec<FileRow> = files
        .iter()
        .filter_map(|file| FileRow::of(&key, file))
        .collect();
    let first = &files[0];
    Ok(VersionDetail {
        id: format!("{key}/{stored}@{version}"),
        repository: key.clone(),
        version,
        created: stamp(&first["created_at"]),
        published_by: text(&first["uploaded_by_username"]),
        size: Some(rows.iter().filter_map(|row| row.size).sum()),
        sha256: match rows.as_slice() {
            [only] => only.sha256.clone(),
            _ => None,
        },
        downloads: Some(rows.iter().filter_map(|row| row.downloads).sum()),
        cached: None,
        yanked: None,
        requires_python: None,
        files: rows,
        package: stored,
    })
}

/// The files Keeper filed under `name` and `version`: their path starts
/// `NAME/VERSION/`, and Keeper read that name and version from them.
fn under(ctx: &Ctx, keeper: &Keeper, key: &str, name: &str, version: &str) -> Result<Vec<Value>> {
    let prefix = format!("{name}/{version}/");
    let files = keeper.artifacts(
        ctx,
        key,
        &[("path_prefix", prefix)],
        crate::client::PAGE_MAX,
    )?;
    Ok(files
        .into_iter()
        .filter(|file| file["version"] == version)
        .collect())
}

fn proxied(
    ctx: &Ctx,
    keeper: &Keeper,
    repo: &Value,
    key: &str,
    name: &str,
    version: &str,
) -> Result<VersionDetail> {
    let release = upstream(ctx, keeper, repo, name, Some(version))?;
    let name = text(&release["info"]["name"]).unwrap_or_else(|| name.to_owned());
    let version = text(&release["info"]["version"]).unwrap_or_else(|| version.to_owned());
    let held = cached(ctx, keeper, key, &name)?;
    let urls = list(&release["urls"]);
    let mut files: Vec<FileRow> = urls
        .iter()
        .filter_map(|file| {
            let filename = text(&file["filename"])?;
            let at = held.get(&filename);
            Some(FileRow {
                id: format!("{key}/simple/{}/{filename}", normalize(&name)),
                size: file["size"].as_u64(),
                sha256: text(&file["digests"]["sha256"]),
                content_type: None,
                created: stamp(&file["upload_time_iso_8601"]),
                published_by: None,
                downloads: None,
                cached: Some(at.is_some()),
                cached_at: at.cloned().flatten(),
                quarantine: None,
            })
        })
        .collect();
    files.sort_by(|a, b| a.created.cmp(&b.created));
    Ok(VersionDetail {
        id: format!("{key}/{name}@{version}"),
        repository: key.to_owned(),
        created: files.first().and_then(|file| file.created.clone()),
        published_by: None,
        size: Some(files.iter().filter_map(|file| file.size).sum()),
        sha256: match files.as_slice() {
            [only] => only.sha256.clone(),
            _ => None,
        },
        downloads: None,
        cached: Some(files.iter().any(|file| file.cached == Some(true))),
        yanked: (!urls.is_empty() && urls.iter().all(|file| file["yanked"] == true))
            .then_some(true),
        requires_python: urls.iter().find_map(|file| text(&file["requires_python"])),
        files,
        package: name,
        version,
    })
}

command! {
    pub VERSION_GET = ["keeper", "version", "get"], Read,
    "Show a package version: when and by whom it was published, checksum, its files",
    keywords: ["release", "published", "uploaded", "who", "when", "checksum", "sha256", "wheel", "build", "cached"],
    example: "keeper version get pypi/contoso-orders-client@2.3.0 --fields created,published_by,files",
    run: version_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{API, BASE, artifact, files, keeper_run, page, repository, urls};

    #[test]
    fn a_hosted_version_is_its_files_with_who_published_them() {
        let mut first = artifact("builds", "api/1.4.2/api-1.4.2-linux-x64.tar.gz", "ab12");
        first["created_at"] = json!("2026-09-28T21:30:01Z");
        let second = artifact(
            "builds",
            "api/1.4.2/api-1.4.2-linux-x64.tar.gz.sha256",
            "cd34",
        );
        let (outcome, transport) = keeper_run(
            &["keeper", "version", "get", "builds/api@1.4.2"],
            vec![
                Answer::json(&repository("builds", "generic", "local")),
                files(vec![second, first], None),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport)[1],
            format!("{API}/repositories/builds/artifacts?path_prefix=api%2F1.4.2%2F&per_page=100")
        );
        let printed = outcome.json();
        assert_eq!(printed["id"], "builds/api@1.4.2");
        assert_eq!(printed["created"], "2026-09-28T21:30:01Z");
        assert_eq!(printed["published_by"], "svc-pipelines");
        assert_eq!(printed["size"], 4096);
        assert_eq!(printed["sha256"], json!(null));
        assert_eq!(
            printed["files"][0]["id"],
            "builds/api/1.4.2/api-1.4.2-linux-x64.tar.gz"
        );
    }

    #[test]
    fn a_version_not_there_is_exit_4_after_asking_the_package_its_name() {
        let package = json!({"id": "00000000-0000-4000-8000-00000000c001", "repository_key": "pypi",
            "name": "contoso-orders-client", "version": "2.3.0", "format": "pypi"});
        let (outcome, transport) = keeper_run(
            &[
                "keeper",
                "version",
                "get",
                "pypi/Contoso_Orders_Client@9.9.9",
            ],
            vec![
                Answer::json(&repository("pypi", "pypi", "local")),
                files(vec![], None),
                page(vec![package], 1),
                files(vec![], None),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert_eq!(
            urls(&transport)[3],
            format!(
                "{API}/repositories/pypi/artifacts?path_prefix=contoso-orders-client%2F9.9.9%2F&per_page=100"
            )
        );
        assert!(
            outcome
                .stderr
                .contains("agent-cli keeper package get pypi/contoso-orders-client"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_proxy_version_is_upstreams_files_marked_cached_or_not() {
        let release = json!({"info": {"name": "pyarrow", "version": "17.0.0"}, "urls": [
            {"filename": "pyarrow-17.0.0.tar.gz", "size": 1000, "digests": {"sha256": "s1"},
             "upload_time_iso_8601": "2024-07-17T10:00:02.000000Z", "yanked": false, "requires_python": ">=3.8"},
            {"filename": "pyarrow-17.0.0-cp311-cp311-manylinux_2_28_x86_64.whl", "size": 2000,
             "digests": {"sha256": "s2"}, "upload_time_iso_8601": "2024-07-17T10:00:01.000000Z",
             "yanked": false, "requires_python": ">=3.8"}]});
        let mut held = artifact("pypi-proxy", "simple/pyarrow/pyarrow-17.0.0.tar.gz", "s1");
        held["cache_cached_at"] = json!("2026-09-20T08:00:00Z");
        let (outcome, transport) = keeper_run(
            &[
                "keeper",
                "version",
                "get",
                &format!("{BASE}/pypi/pypi-proxy/pypi/pyarrow/17.0.0/json"),
            ],
            vec![
                Answer::json(&repository("pypi-proxy", "pypi", "remote")),
                Answer::json(&release),
                files(vec![held], None),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport)[1],
            format!("{BASE}/pypi/pypi-proxy/pypi/pyarrow/17.0.0/json")
        );
        let printed = outcome.json();
        assert_eq!(printed["cached"], true);
        assert_eq!(printed["created"], "2024-07-17T10:00:01Z");
        assert_eq!(
            printed["files"],
            json!([{"id": "pypi-proxy/simple/pyarrow/pyarrow-17.0.0-cp311-cp311-manylinux_2_28_x86_64.whl",
                    "size": 2000, "sha256": "s2", "created": "2024-07-17T10:00:01Z", "cached": false},
                   {"id": "pypi-proxy/simple/pyarrow/pyarrow-17.0.0.tar.gz", "size": 1000, "sha256": "s1",
                    "created": "2024-07-17T10:00:02Z", "cached": true, "cached_at": "2026-09-20T08:00:00Z"}])
        );
    }
}
