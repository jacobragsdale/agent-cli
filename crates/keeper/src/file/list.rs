//! `keeper file list`: `GET /repositories/{key}/artifacts` under a path
//! prefix, by Keeper's keyset cursor, in path order.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use super::FileRow;
use crate::client::{Keeper, note_more};
use crate::ids;

#[derive(clap::Args)]
pub struct FileListArgs {
    /// REPO, or REPO/PREFIX for the files whose path starts with PREFIX (api/1.4.2/), or a path with --repo
    folder: String,
    /// The repository, when FOLDER is a bare path prefix
    #[arg(long)]
    repo: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn file_list(ctx: &Ctx, args: FileListArgs) -> Result<Vec<FileRow>> {
    let keeper = Keeper::load(ctx)?;
    let folder = args.folder.trim_end_matches('/');
    let (key, mut prefix) = match ids::file(&keeper, folder, args.repo.as_deref()) {
        Ok(found) => found,
        Err(_) if args.repo.is_none() => (ids::repo(&keeper, folder)?, String::new()),
        Err(error) => return Err(error),
    };
    let mut query = Vec::new();
    if !prefix.is_empty() {
        // `api/1.4.2/` is that folder; `api/1.4.2` would match 1.4.20 too.
        if args.folder.ends_with('/') {
            prefix.push('/');
        }
        query.push(("path_prefix", prefix));
    }
    let found = keeper.artifacts(ctx, &key, &query, args.limit)?;
    let mut rows: Vec<FileRow> = found
        .iter()
        .filter_map(|file| FileRow::of(&key, file))
        .collect();
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        note_more(ctx, args.limit, None);
    }
    Ok(rows)
}

command! {
    pub FILE_LIST = ["keeper", "file", "list"], Read,
    "List stored files under a path prefix, with size, checksum and uploader",
    keywords: ["artifacts", "build", "generic", "binaries", "uploads", "uploaded", "folder", "directory", "browse", "contents"],
    example: "keeper file list builds/api/1.4.2/ --fields id,size,created,published_by",
    run: file_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{API, artifact, files, keeper_run, urls};

    #[test]
    fn files_list_under_a_prefix_and_follow_the_cursor() {
        let (outcome, transport) = keeper_run(
            &[
                "keeper",
                "file",
                "list",
                "builds/api/1.4.2/",
                "--limit",
                "2",
            ],
            vec![
                files(
                    vec![artifact(
                        "builds",
                        "api/1.4.2/api-1.4.2-linux-x64.tar.gz",
                        "ab",
                    )],
                    Some("c1"),
                ),
                files(
                    vec![
                        artifact(
                            "builds",
                            "api/1.4.2/api-1.4.2-linux-x64.tar.gz.sha256",
                            "cd",
                        ),
                        artifact("builds", "api/1.4.2/api-1.4.2-win-x64.zip", "ef"),
                    ],
                    None,
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport),
            [
                format!(
                    "{API}/repositories/builds/artifacts?path_prefix=api%2F1.4.2%2F&per_page=3"
                ),
                format!(
                    "{API}/repositories/builds/artifacts?path_prefix=api%2F1.4.2%2F&per_page=3&cursor=c1"
                ),
            ]
        );
        let printed = outcome.json();
        assert_eq!(printed.as_array().unwrap().len(), 2);
        assert_eq!(
            printed[0],
            json!({"id": "builds/api/1.4.2/api-1.4.2-linux-x64.tar.gz", "size": 2048, "sha256": "ab",
                "content_type": "application/gzip", "created": "2026-09-28T21:30:05Z",
                "published_by": "svc-pipelines", "downloads": 3})
        );
        assert!(
            outcome.stderr.contains("[first 2; --limit N for more]"),
            "{}",
            outcome.stderr
        );
    }
}
