//! Files in a repository at a ref. `fetch` is also how `thread list` and
//! `diff get` read the code they show.

pub(crate) mod get;
pub(crate) mod list;

use std::time::Duration;

use agent_cli_core::{Ctx, Failure, Method};
use anyhow::Result;
use serde_json::Value;

use crate::client::{API, Ado, Body, list, query_value};
use crate::ids::{items_path, version_query};

/// Azure DevOps cuts an item's JSON content at 5 MiB, without a word.
const JSON_CONTENT: usize = 5 * 1024 * 1024;

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
    let path = query_value(&format!("/{}", path.trim_start_matches('/')));
    let version = version_query(reading);
    let query = format!("path={path}&{version}includeContent=true&$format=json");
    let mut item = ado.get(ctx, &ado.api(Some(project), &items_path(repo), &query, API))?;
    // Content that long was cut: the stream is the whole file.
    if item["content"]
        .as_str()
        .is_some_and(|content| content.len() >= JSON_CONTENT)
    {
        let query = format!("path={path}&{version}$format=octetStream");
        let url = ado.api(Some(project), &items_path(repo), &query, API);
        let whole = ado.send(ctx, Method::Get, &url, Body::None, None)?;
        item["content"] = Value::String(match whole.bytes {
            Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            None => whole.body,
        });
    }
    Ok(item)
}

/// A commit's tree never changes.
const FOR_GOOD: Duration = Duration::from_secs(10 * 365 * 24 * 3600);

/// The paths of a repository's files at `reading` (`None` is the default
/// branch), without a leading `/`: what [`resolve`] matches. A commit's are
/// cached, one commit per repository: a tree can be megabytes, and every
/// command reads the one cache file. A branch's or a tag's are read each time.
// ponytail: one read of the whole tree; narrow it with scopePath from the
// path's last segments if a live repository makes it slow.
pub(crate) fn tree(
    ctx: &Ctx,
    ado: &Ado,
    project: &str,
    repo: &str,
    reading: Option<&(&str, String)>,
) -> Result<Vec<String>> {
    let commit = reading
        .filter(|(kind, _)| *kind == "commit")
        .map(|(_, commit)| commit.as_str());
    let key = ado.cache_key(&format!(
        "tree:{}/{}",
        project.to_lowercase(),
        repo.to_lowercase()
    ));
    if let Some(commit) = commit
        && let Some((cached, files)) = ctx.cache().get::<(String, Vec<String>)>(&key)
        && cached == commit
    {
        return Ok(files);
    }
    let query = format!(
        "scopePath=%2F&{}recursionLevel=Full",
        version_query(reading)
    );
    let answer = ado.get(ctx, &ado.api(Some(project), &items_path(repo), &query, API))?;
    let files: Vec<String> = list(&answer["value"])
        .iter()
        .filter(|item| item["isFolder"].as_bool() != Some(true))
        .filter_map(|item| item["path"].as_str())
        .map(|path| path.trim_start_matches('/').to_owned())
        .collect();
    if let Some(commit) = commit {
        ctx.cache().put(&key, &(commit, &files), FOR_GOOD);
    }
    Ok(files)
}

/// The one file in `files` that `path` names: of the suffixes of `path`,
/// longest first, the first that some file ends with, when exactly one
/// does. A build agent's `/home/vsts/work/1/s/src/x.cs`, a multi-repo
/// checkout's `s/api/src/x.cs`, a Dockerfile's `WORKDIR /src` and a
/// container's `/app/src/x.cs` all end in the repository's `src/x.cs`, so
/// none needs configuring. Two files ending the same way are no match: no
/// answer rather than a wrong one.
pub(crate) fn resolve(files: &[String], path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    for start in 0..segments.len() {
        let suffix = segments[start..].join("/");
        let tail = format!("/{suffix}");
        let mut found = files
            .iter()
            .filter(|file| **file == suffix || file.ends_with(&tail));
        match (found.next(), found.next()) {
            (Some(one), None) => return Some(one.clone()),
            (Some(_), Some(_)) => return None,
            _ => {}
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{FakeTransport, ctx};
    use agent_cli_core::{Config, Setup};
    use serde_json::json;

    use super::{resolve, tree};
    use crate::client::Ado;
    use crate::testing::page;

    #[test]
    fn a_path_resolves_to_the_one_file_it_ends_with_and_an_ambiguous_one_to_none() {
        let files: Vec<String> = [
            "src/Orders/OrderClient.cs",
            "src/Api/Program.cs",
            "tools/Seed/Program.cs",
            "README.md",
        ]
        .map(str::to_owned)
        .to_vec();
        for printed in [
            "/home/vsts/work/1/s/src/Orders/OrderClient.cs",
            r"D:\a\1\s\api\src\Orders\OrderClient.cs",
            "/app/src/Orders/OrderClient.cs",
            "/src/Orders/OrderClient.cs",
            "src/Orders/OrderClient.cs",
            "/src/OrderClient.cs",
        ] {
            assert_eq!(
                resolve(&files, printed).as_deref(),
                Some("src/Orders/OrderClient.cs"),
                "{printed}"
            );
        }
        assert_eq!(
            resolve(&files, "/app/src/Api/Program.cs").as_deref(),
            Some("src/Api/Program.cs")
        );
        assert_eq!(resolve(&files, "/app/Program.cs"), None, "two Program.cs");
        assert_eq!(resolve(&files, "/usr/lib/dotnet/System.cs"), None);
    }

    #[test]
    fn one_commit_tree_per_repository_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let answer = || page(vec![json!({"path": "/src/x.cs"})]);
        let transport = FakeTransport::answering([answer(), answer(), answer()]);
        let ctx = ctx(Setup {
            cache_dir: Some(dir.path().to_owned()),
            ..Setup::fake(transport.clone())
        });
        let config = "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\n";
        let ado = Ado::names(&Config::parse("c.toml", Some(config), Vec::new())).unwrap();
        let read = |commit: &str| {
            let reading = ("commit", commit.to_owned());
            tree(&ctx, &ado, "Fabrikam", "web", Some(&reading)).unwrap()
        };
        assert_eq!(read("c0ffee1"), ["src/x.cs"]);
        assert_eq!(read("c0ffee1"), ["src/x.cs"]);
        assert_eq!(transport.sent().len(), 1);
        read("beef002");
        read("c0ffee1");
        assert_eq!(transport.sent().len(), 3, "the newer commit took its place");
        let stored = std::fs::read_to_string(dir.path().join("cache.json")).unwrap();
        assert_eq!(stored.matches("tree:").count(), 1, "{stored}");
    }
}
