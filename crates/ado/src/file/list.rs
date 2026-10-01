use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{API, Ado, list, query_value, text};
use crate::ids::{FileId, file_id, items_path, resolving, version_query};

#[derive(clap::Args)]
pub struct FileListArgs {
    /// The folder: REPO[@REF][:PATH] (the root without a path), as file list prints it, or its web URL
    folder: String,
    /// The branch, tag or commit (default: the repository's default branch)
    #[arg(long = "ref")]
    reference: Option<String>,
    /// Everything under the folder, not only what is in it
    #[arg(long)]
    recursive: bool,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A file or folder in a repository.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FileRow {
    /// What file get (a file) or file list (a folder) takes.
    id: String,
    path: String,
    /// file or folder.
    kind: &'static str,
}

fn file_list(ctx: &Ctx, args: FileListArgs) -> Result<Vec<FileRow>> {
    let ado = Ado::load(ctx)?;
    let folder = FileId::parse(&ado, &args.folder, args.reference.as_deref(), None)?;
    let refs: Vec<&str> = folder.reference.as_deref().into_iter().collect();
    let scope = format!("/{}", folder.path);
    let answer = resolving(&refs, |reading| {
        let query = format!(
            "scopePath={}&{}recursionLevel={}",
            query_value(&scope),
            version_query(reading.first()),
            if args.recursive { "Full" } else { "OneLevel" },
        );
        let url = ado.api(
            Some(folder.project(&ado)),
            &items_path(&folder.repo),
            &query,
            API,
        );
        ado.get(ctx, &url)
    })?;
    let mut rows: Vec<FileRow> = list(&answer["value"])
        .iter()
        .filter_map(|item| {
            let path = text(&item["path"])?.trim_start_matches('/').to_owned();
            (path != folder.path).then(|| FileRow {
                id: file_id(
                    folder.project.as_deref(),
                    &folder.repo,
                    folder.reference.as_deref(),
                    &path,
                    None,
                ),
                kind: if item["isFolder"].as_bool() == Some(true) {
                    "folder"
                } else {
                    "file"
                },
                path,
            })
        })
        .collect();
    if rows.len() > args.limit {
        ctx.note(format!(
            "[first {} of {}; --limit N for more]",
            args.limit,
            rows.len()
        ));
        rows.truncate(args.limit);
    }
    Ok(rows)
}

command! {
    pub FILE_LIST = ["ado", "file", "list"], Read,
    "List the files and folders in a repository folder at a branch, tag or commit",
    keywords: ["ls", "tree", "directory", "browse", "contents", "children"],
    example: "ado file list worker:src/Jobs --fields id,kind",
    run: file_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, urls};

    #[test]
    fn a_folder_lists_its_children_as_ids_file_get_takes() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "file",
                "list",
                "worker@main:src/Jobs",
                "--limit",
                "2",
            ],
            vec![Answer::json(&json!({"count": 4, "value": [
                {"path": "/src/Jobs", "isFolder": true, "gitObjectType": "tree"},
                {"path": "/src/Jobs/Retry.cs", "gitObjectType": "blob"},
                {"path": "/src/Jobs/Sweep.cs", "gitObjectType": "blob"},
                {"path": "/src/Jobs/Internal", "isFolder": true, "gitObjectType": "tree"},
            ]}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "worker@main:src/Jobs/Retry.cs", "path": "src/Jobs/Retry.cs", "kind": "file"},
                {"id": "worker@main:src/Jobs/Sweep.cs", "path": "src/Jobs/Sweep.cs", "kind": "file"},
            ])
        );
        assert!(
            outcome
                .stderr
                .contains("[first 2 of 3; --limit N for more]")
        );
        assert_eq!(
            urls(&transport),
            [format!(
                "{CODE}/git/repositories/worker/items?scopePath=%2Fsrc%2FJobs&versionDescriptor.version=main&versionDescriptor.versionType=branch&recursionLevel=OneLevel&api-version=7.1"
            )]
        );
    }

    #[test]
    fn a_repository_alone_is_its_root_and_recursive_walks_everything() {
        let (outcome, transport) = ado(
            &["ado", "file", "list", "Data/etl", "--recursive"],
            vec![Answer::json(
                &json!({"value": [{"path": "/", "isFolder": true}, {"path": "/dags/a.py"}]}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "Data/etl:dags/a.py", "path": "dags/a.py", "kind": "file"}])
        );
        assert_eq!(
            urls(&transport),
            [
                "https://dev.azure.com/contoso/Data/_apis/git/repositories/etl/items?scopePath=%2F&recursionLevel=Full&api-version=7.1"
            ]
        );
    }
}
