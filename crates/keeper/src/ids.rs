//! The ids rows print and the verbs take, and the URLs that name the same
//! things: `REPO`, `REPO/PACKAGE`, `REPO/PACKAGE@VERSION` and `REPO/PATH`.
//!
//! A URL is parsed, never fetched: the web UI's `/repositories/KEY` and
//! `/packages/ID`, the API's `/api/v1/repositories/KEY[/download|/artifacts/PATH]`,
//! and a PyPI index's `/pypi/KEY/simple/[NAME/]` (what pip's `index-url`
//! and its errors show). A URL off the configured server is exit 2.

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;

use crate::client::{Keeper, segment, text};

/// What a ref names.
#[derive(Debug, PartialEq, Eq)]
enum Named {
    Repo(String),
    Package(String, String),
    /// The web UI's `/packages/ID`, with the repository it was opened in.
    PackageId(String, Option<String>),
    Version(String, String, String),
    File(String, String),
}

/// A repository's key, from `KEY` or a URL that names it.
pub(crate) fn repo(keeper: &Keeper, raw: &str) -> Result<String> {
    match parse_url(keeper, raw)? {
        Some(Named::Repo(key)) => Ok(key),
        Some(_) => Err(not_a(raw, "repository", "agent-cli keeper repo list")),
        None if raw.contains('/') || raw.is_empty() => {
            Err(not_a(raw, "repository key", "agent-cli keeper repo list"))
        }
        None => Ok(raw.to_owned()),
    }
}

/// `(repository, package)` from `REPO/PACKAGE`, `PACKAGE` with `--repo`, or
/// a URL; a web UI package URL is looked up by its id.
pub(crate) fn package(
    keeper: &Keeper,
    ctx: &Ctx,
    raw: &str,
    flag: Option<&str>,
) -> Result<(String, String)> {
    let (repo, name) = match parse_url(keeper, raw)? {
        Some(Named::Package(repo, name)) => (Some(repo), name),
        Some(Named::PackageId(id, opened_in)) => {
            let found = keeper.get(
                ctx,
                &keeper.api(&format!("/packages/{}", segment(&id)), &[]),
            )?;
            let name = text(&found["name"]).unwrap_or_default();
            (opened_in.or_else(|| text(&found["repository_key"])), name)
        }
        Some(Named::Version(..)) => {
            return Err(not_a(
                raw,
                "package",
                &format!("agent-cli keeper version get {raw}"),
            ));
        }
        Some(_) => return Err(not_a(raw, "package", "agent-cli keeper package list REPO")),
        None if raw.contains('@') => {
            return Err(not_a(
                raw,
                "package",
                &format!("agent-cli keeper version get {raw}"),
            ));
        }
        None => match raw.split_once('/') {
            Some((repo, name)) => (Some(repo.to_owned()), name.to_owned()),
            None => (None, raw.to_owned()),
        },
    };
    let repo = agree(raw, repo, flag)?;
    if name.is_empty() || name.contains('/') {
        return Err(not_a(raw, "package", "agent-cli keeper package list REPO"));
    }
    Ok((repo, name))
}

/// `(repository, package, version)` from `REPO/PACKAGE@VERSION` (or
/// `PACKAGE@VERSION` with `--repo`) or a PyPI release's JSON URL.
pub(crate) fn version(
    keeper: &Keeper,
    raw: &str,
    flag: Option<&str>,
) -> Result<(String, String, String)> {
    let (repo, name, version) = match parse_url(keeper, raw)? {
        Some(Named::Version(repo, name, version)) => (Some(repo), name, version),
        Some(_) => {
            return Err(not_a(
                raw,
                "version",
                "agent-cli keeper package get REPO/PACKAGE",
            ));
        }
        None => {
            let Some((package, version)) = raw.rsplit_once('@') else {
                return Err(Failure::usage(format!(
                    "{raw} names no version: a version is REPO/PACKAGE@VERSION"
                ))
                .hint(format!("agent-cli keeper package get {raw}"))
                .into());
            };
            match package.split_once('/') {
                Some((repo, name)) => (Some(repo.to_owned()), name.to_owned(), version.to_owned()),
                None => (None, package.to_owned(), version.to_owned()),
            }
        }
    };
    let repo = agree(raw, repo, flag)?;
    if name.is_empty() || name.contains('/') || version.is_empty() {
        return Err(not_a(
            raw,
            "version",
            "agent-cli keeper package get REPO/PACKAGE",
        ));
    }
    Ok((repo, name, version))
}

/// `(repository, path)` from `REPO/PATH`, `PATH` with `--repo`, or a
/// download or artifact URL. With `--repo`, a ref that starts with that
/// repository's key and a slash is taken as `REPO/PATH`.
pub(crate) fn file(keeper: &Keeper, raw: &str, flag: Option<&str>) -> Result<(String, String)> {
    let (repo, path) = match parse_url(keeper, raw)? {
        Some(Named::File(repo, path)) => (Some(repo), path),
        Some(_) => return Err(not_a(raw, "file", "agent-cli keeper file list REPO")),
        None => match flag {
            Some(flag) => match raw
                .strip_prefix(flag)
                .and_then(|rest| rest.strip_prefix('/'))
            {
                Some(path) => (Some(flag.to_owned()), path.to_owned()),
                None => (None, raw.to_owned()),
            },
            None => match raw.split_once('/') {
                Some((repo, path)) => (Some(repo.to_owned()), path.to_owned()),
                None => (None, raw.to_owned()),
            },
        },
    };
    let repo = agree(raw, repo, flag)?;
    let path = path.trim_matches('/').to_owned();
    if path.is_empty() {
        return Err(not_a(
            raw,
            "file",
            &format!("agent-cli keeper file list {repo}"),
        ));
    }
    Ok((repo, path))
}

/// The repository a ref names and the one `--repo` names must be the same;
/// one of them must name it.
fn agree(raw: &str, named: Option<String>, flag: Option<&str>) -> Result<String> {
    match (named, flag) {
        (Some(named), Some(flag)) if named != flag => Err(Failure::usage(format!(
            "{raw} is in repository {named}, and --repo says {flag}"
        ))
        .hint("drop --repo, or the repository from the ref")
        .into()),
        (Some(named), _) => Ok(named),
        (None, Some(flag)) => Ok(flag.to_owned()),
        (None, None) => Err(Failure::usage(format!(
            "{raw} names no repository: write REPO/{raw}, or add --repo REPO"
        ))
        .hint("agent-cli keeper repo list")
        .into()),
    }
}

fn not_a(raw: &str, what: &str, hint: &str) -> anyhow::Error {
    Failure::usage(format!("{raw} does not name a {what}"))
        .hint(hint)
        .into()
}

/// What a URL under the server names; `None` when `raw` is not a URL.
fn parse_url(keeper: &Keeper, raw: &str) -> Result<Option<Named>> {
    if !(raw.starts_with("https://") || raw.starts_with("http://")) {
        return Ok(None);
    }
    let base = &keeper.base;
    let Some(rest) = raw
        .get(..base.len())
        .filter(|head| head.eq_ignore_ascii_case(base))
        .map(|_| &raw[base.len()..])
        .filter(|rest| rest.is_empty() || rest.starts_with(['/', '?', '#']))
    else {
        return Err(
            Failure::usage(format!("{raw} is not on the configured Keeper, {base}"))
                .hint("agent-cli config")
                .into(),
        );
    };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let path = path.split('#').next().unwrap_or_default();
    let parts: Vec<String> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(decode)
        .collect();
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    let param = |key: &str| {
        query.split('&').find_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            (name == key && !value.is_empty()).then(|| decode(value))
        })
    };
    let named = match parts.as_slice() {
        ["repositories", key] | ["api", "v1", "repositories", key] => {
            Named::Repo((*key).to_owned())
        }
        ["packages", id] => Named::PackageId((*id).to_owned(), param("repository_key")),
        ["packages"] if param("selected").is_some() => {
            Named::PackageId(param("selected").unwrap_or_default(), param("repository"))
        }
        [
            "api",
            "v1",
            "repositories",
            key,
            "download" | "artifacts",
            path @ ..,
        ] if !path.is_empty() => Named::File((*key).to_owned(), path.join("/")),
        ["pypi", key] | ["pypi", key, "simple"] => Named::Repo((*key).to_owned()),
        ["pypi", key, "simple", name] | ["pypi", key, "pypi", name, "json"] => {
            Named::Package((*key).to_owned(), (*name).to_owned())
        }
        ["pypi", key, "pypi", name, version, "json"] => {
            Named::Version((*key).to_owned(), (*name).to_owned(), (*version).to_owned())
        }
        _ => {
            return Err(Failure::usage(format!(
                "{raw} is not a Keeper repository, package or file URL"
            ))
            .hint("agent-cli keeper repo list")
            .into());
        }
    };
    Ok(Some(named))
}

/// `%XX` escapes decoded; anything malformed stays as it was.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| (bytes.get(at).copied().map(char::from)?).to_digit(16);
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (byte, ..) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{BASE, keeper};

    #[test]
    fn a_url_on_the_server_names_what_its_path_shows() {
        let keeper = keeper();
        let url = |path: &str| {
            parse_url(&keeper, &format!("{BASE}{path}"))
                .unwrap()
                .unwrap()
        };
        assert_eq!(url("/repositories/pypi"), Named::Repo("pypi".into()));
        assert_eq!(
            url("/api/v1/repositories/builds"),
            Named::Repo("builds".into())
        );
        assert_eq!(
            url("/pypi/pypi-all/simple/"),
            Named::Repo("pypi-all".into())
        );
        assert_eq!(
            url("/pypi/pypi-proxy/simple/pyarrow/"),
            Named::Package("pypi-proxy".into(), "pyarrow".into())
        );
        assert_eq!(
            url("/pypi/pypi/pypi/contoso-orders-client/2.3.0/json"),
            Named::Version(
                "pypi".into(),
                "contoso-orders-client".into(),
                "2.3.0".into()
            )
        );
        assert_eq!(
            url("/packages/00000000-0000-4000-8000-00000000c001?repository_key=pypi-all"),
            Named::PackageId(
                "00000000-0000-4000-8000-00000000c001".into(),
                Some("pypi-all".into())
            )
        );
        assert_eq!(
            url("/api/v1/repositories/builds/download/api/1.4.2/api%201.4.2.tar.gz"),
            Named::File("builds".into(), "api/1.4.2/api 1.4.2.tar.gz".into())
        );
        for wrong in [
            "https://keeper.fabrikam.example/repositories/pypi",
            "https://keeper.contoso.example.evil/repositories/pypi",
            "https://keeper.contoso.example/builds",
        ] {
            assert!(parse_url(&keeper, wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_ref_and_repo_must_agree_and_one_must_name_the_repository() {
        let keeper = keeper();
        assert_eq!(
            version(&keeper, "pypi/contoso-orders-client@2.3.0", None).unwrap(),
            (
                "pypi".into(),
                "contoso-orders-client".into(),
                "2.3.0".into()
            )
        );
        assert_eq!(
            version(&keeper, "contoso-orders-client@2.3.0", Some("pypi"))
                .unwrap()
                .0,
            "pypi"
        );
        assert!(version(&keeper, "pypi/contoso-orders-client@2.3.0", Some("builds")).is_err());
        assert!(version(&keeper, "contoso-orders-client@2.3.0", None).is_err());
        assert!(version(&keeper, "pypi/contoso-orders-client", None).is_err());
        assert_eq!(
            file(&keeper, "builds/api/1.4.2/api.tar.gz", None).unwrap(),
            ("builds".into(), "api/1.4.2/api.tar.gz".into())
        );
        assert_eq!(
            file(&keeper, "api/1.4.2/api.tar.gz", Some("builds")).unwrap(),
            ("builds".into(), "api/1.4.2/api.tar.gz".into())
        );
        assert_eq!(
            file(&keeper, "builds/api/1.4.2/api.tar.gz", Some("builds"))
                .unwrap()
                .1,
            "api/1.4.2/api.tar.gz"
        );
        assert!(repo(&keeper, "pypi/contoso-orders-client").is_err());
    }
}
