//! `keeper file get`: a file's metadata (`GET /repositories/{key}/artifacts/{path}`)
//! and, with `--output`, its bytes (`GET /repositories/{key}/download/{path}`).

use agent_cli_core::{Ctx, Exit, Failure, command, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Keeper, quarantine, segment, stamp, text};
use crate::ids;

#[derive(clap::Args)]
pub struct FileGetArgs {
    /// The file: REPO/PATH, as file list and version get print it, PATH with --repo, or its download URL. --output FILE saves its bytes
    file: String,
    /// The repository, when FILE is a bare path
    #[arg(long)]
    repo: Option<String>,
}

/// One file, and where `--output` saved it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FileDetail {
    /// REPO/PATH: what file get takes.
    id: String,
    repository: String,
    path: String,
    /// The package and version Keeper read from the upload, when it did.
    package: Option<String>,
    version: Option<String>,
    /// Bytes.
    size: Option<u64>,
    sha256: Option<String>,
    content_type: Option<String>,
    created: Option<String>,
    /// Who uploaded it: a user or service account.
    published_by: Option<String>,
    downloads: Option<u64>,
    /// hosted (uploaded here) or proxy (fetched from upstream).
    origin: Option<String>,
    /// A proxy's file: when it was fetched, and when it is asked for again.
    cached_at: Option<String>,
    cache_expires: Option<String>,
    /// Held back from downloads for review: quarantined, pending …
    quarantine: Option<String>,
    /// Where --output wrote the bytes.
    saved: Option<String>,
}

fn file_get(ctx: &Ctx, args: FileGetArgs) -> Result<FileDetail> {
    let keeper = Keeper::load(ctx)?;
    let (key, path) = ids::file(&keeper, &args.file, args.repo.as_deref())?;
    let url = keeper.api(
        &format!(
            "/repositories/{}/artifacts/{}",
            segment(&key),
            segment(&path)
        ),
        &[],
    );
    let found = match keeper.get(ctx, &url) {
        Err(error) if status_of(&error) == Some(404) => {
            // The repository says which miss it is: none (or private), a
            // proxy that has not fetched the file, or a path not uploaded.
            let repo = keeper.repo(ctx, &key)?;
            let proxy = repo["repo_type"] == "remote";
            // A proxy's cached files have no row of their own (Keeper lists
            // them with made-up ids), so the listing is where they show.
            let listed = match proxy {
                true => keeper.artifacts(ctx, &key, &[("path_prefix", path.clone())], 10)?,
                false => Vec::new(),
            };
            if let Some(found) = listed
                .into_iter()
                .find(|file| file["path"] == path.as_str())
            {
                return detail(ctx, &keeper, &key, &path, &found);
            }
            let what = match proxy {
                true => format!(
                    "{key} has not cached {path}: a proxy holds a file once something downloads it through the proxy"
                ),
                false => format!("no file {path} in {key}"),
            };
            let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
            return Err(Failure::not_found(what)
                .hint(match folder {
                    "" => format!("agent-cli keeper file list {key}"),
                    folder => format!("agent-cli keeper file list {key}/{folder}/"),
                })
                .into());
        }
        found => found?,
    };
    detail(ctx, &keeper, &key, &path, &found)
}

/// The row for `found`, and its bytes saved under `--output`.
fn detail(ctx: &Ctx, keeper: &Keeper, key: &str, path: &str, found: &Value) -> Result<FileDetail> {
    let mut detail = FileDetail {
        id: format!("{key}/{path}"),
        package: found["version"].as_str().and_then(|_| text(&found["name"])),
        version: text(&found["version"]),
        size: found["size_bytes"].as_u64(),
        sha256: text(&found["checksum_sha256"]),
        content_type: text(&found["content_type"]),
        created: stamp(&found["created_at"]),
        published_by: text(&found["uploaded_by_username"]),
        downloads: found["download_count"].as_u64(),
        origin: text(&found["origin"]["kind"]),
        cached_at: stamp(&found["cache_cached_at"]),
        cache_expires: stamp(&found["cache_expires_at"]),
        quarantine: quarantine(found),
        saved: None,
        repository: key.to_owned(),
        path: path.to_owned(),
    };
    if ctx.globals().output.is_none() {
        return Ok(detail);
    }
    let url = keeper.api(
        &format!("/repositories/{}/download/{}", segment(key), segment(path)),
        &[],
    );
    let bytes = keeper.fetch(ctx, &url)?.into_bytes();
    if let Some(size) = detail.size
        && size != bytes.len() as u64
    {
        return Err(Failure::new(
            Exit::Failed,
            format!(
                "{key}/{path} came as {} bytes where Keeper records {size}; nothing was saved",
                bytes.len()
            ),
        )
        .hint("run it again")
        .into());
    }
    if let Some(saved) = ctx.save(&bytes)? {
        detail.saved = Some(saved.display().to_string());
    }
    Ok(detail)
}

command! {
    pub FILE_GET = ["keeper", "file", "get"], Read,
    "Show a file's size, checksum and uploader, or download it with --output",
    keywords: ["download", "artifact", "fetch", "save", "binary", "output", "sha256", "checksum", "tarball"],
    example: "keeper file get builds/api/1.4.2/api-1.4.2-linux-x64.tar.gz --output api.tar.gz",
    // Keeper throttles downloads per address (30 a minute by default) and
    // asks for 60 s: the wait fits.
    timeout: 100,
    run: file_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{
        API, BASE, TOKEN, anonymous, artifact, files, keeper_run, repository, urls,
    };

    const PATH: &str = "api/1.4.2/api-1.4.2-linux-x64.tar.gz";

    #[test]
    fn a_file_prints_its_checksum_and_uploader_without_downloading() {
        let (outcome, transport) = keeper_run(
            &["keeper", "file", "get", &format!("builds/{PATH}")],
            vec![Answer::json(&artifact("builds", PATH, "ab12"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport),
            [format!("{API}/repositories/builds/artifacts/{PATH}")]
        );
        assert_eq!(
            outcome.json(),
            json!({"id": format!("builds/{PATH}"), "repository": "builds", "path": PATH, "package": "api",
                "version": "1.4.2", "size": 2048, "sha256": "ab12", "content_type": "application/gzip",
                "created": "2026-09-28T21:30:05Z", "published_by": "svc-pipelines", "downloads": 3,
                "origin": "hosted"})
        );
    }

    #[test]
    fn output_saves_the_bytes_from_the_download_url() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("api.tar.gz");
        let bytes = vec![0x1f_u8; 2048];
        let (outcome, transport) = keeper_run(
            &[
                "keeper",
                "file",
                "get",
                &format!("{BASE}/api/v1/repositories/builds/download/{PATH}"),
                "--output",
                saved.to_str().unwrap(),
            ],
            vec![
                Answer::json(&artifact("builds", PATH, "ab12")),
                Answer::bytes(&bytes),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport)[1],
            format!("{API}/repositories/builds/download/{PATH}")
        );
        assert_eq!(
            transport.sent()[1].authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        assert_eq!(outcome.json()["saved"], saved.to_str().unwrap());

        let (outcome, _) = keeper_run(
            &[
                "keeper",
                "file",
                "get",
                &format!("builds/{PATH}"),
                "--output",
                saved.to_str().unwrap(),
            ],
            vec![
                Answer::json(&artifact("builds", PATH, "ab12")),
                Answer::bytes(b"short"),
            ],
        );
        assert_eq!(
            outcome.code, 1,
            "a short download is not saved: {outcome:?}"
        );
    }

    #[test]
    fn a_missing_file_names_its_folder_and_a_throttled_download_waits_or_says_so() {
        let (outcome, _) = anonymous(
            &[
                "keeper",
                "file",
                "get",
                "pypi-proxy/simple/six/six-1.17.0.tar.gz",
            ],
            vec![
                Answer::status(
                    404,
                    r#"{"code":"NOT_FOUND","message":"Artifact not found"}"#,
                ),
                Answer::json(&repository("pypi-proxy", "pypi", "remote")),
                files(
                    vec![artifact(
                        "pypi-proxy",
                        "simple/six/six-1.17.0.tar.gz.asc",
                        "00",
                    )],
                    None,
                ),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("pypi-proxy has not cached"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("agent-cli keeper file list pypi-proxy/simple/six/"),
            "{}",
            outcome.stderr
        );
        let mut cached = artifact("pypi-proxy", "simple/six/six-1.17.0.tar.gz", "s1");
        cached["cache_cached_at"] = serde_json::json!("2026-09-20T08:00:00Z");
        let (outcome, transport) = anonymous(
            &[
                "keeper",
                "file",
                "get",
                "pypi-proxy/simple/six/six-1.17.0.tar.gz",
            ],
            vec![
                Answer::status(
                    404,
                    r#"{"code":"NOT_FOUND","message":"Artifact not found"}"#,
                ),
                Answer::json(&repository("pypi-proxy", "pypi", "remote")),
                files(vec![cached], None),
            ],
        );
        assert_eq!(
            outcome.code, 0,
            "a cached file shows from the listing: {outcome:?}"
        );
        assert_eq!(
            urls(&transport)[2],
            format!(
                "{API}/repositories/pypi-proxy/artifacts?path_prefix=simple%2Fsix%2Fsix-1.17.0.tar.gz&per_page=11"
            )
        );
        assert_eq!(outcome.json()["cached_at"], "2026-09-20T08:00:00Z");
        let (outcome, _) = anonymous(
            &["keeper", "file", "get", "internal/etl/2.0.1/etl-2.0.1.whl"],
            vec![
                Answer::status(
                    404,
                    r#"{"code":"NOT_FOUND","message":"Artifact not found"}"#,
                ),
                Answer::status(
                    404,
                    r#"{"code":"NOT_FOUND","message":"Repository not found"}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("it is private"),
            "{}",
            outcome.stderr
        );
        let (outcome, _) = anonymous(
            &[
                "keeper",
                "file",
                "get",
                "internal/etl/2.0.1/etl-2.0.1.whl",
                "--output",
                "/dev/null",
                "--timeout",
                "5",
            ],
            vec![
                Answer::json(&artifact("internal", "etl/2.0.1/etl-2.0.1.whl", "ff")),
                Answer::status(429, "Rate limit exceeded. Please try again later.")
                    .with_header("Retry-After", "60"),
            ],
        );
        assert_eq!(
            outcome.code, 124,
            "the wait does not fit in 5 s: {outcome:?}"
        );
    }
}
