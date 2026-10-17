//! `lake path list`: what is in a folder, one level or all of it, with the
//! totals in a note. Path List has no time filter, so `--since` and
//! `--until` filter what it read.

use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::lake::table::stamp;
use crate::lake::{Lake, bytes};

#[derive(clap::Args)]
pub struct PathListArgs {
    /// The container or folder: an id (ACCOUNT/CONTAINER/PATH) or its abfss://, wasbs:// or https:// URL
    path: String,
    /// Everything below it, not one level
    #[arg(long)]
    recursive: bool,
    /// Only what changed since then
    #[arg(long)]
    since: Option<When>,
    /// Only what changed before then
    #[arg(long)]
    until: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PathRow {
    /// `account/container/path`, which `lake path get` takes.
    id: String,
    /// file or directory.
    kind: &'static str,
    size: u64,
    modified: Option<String>,
    owner: Option<String>,
}

fn path_list(ctx: &Ctx, args: PathListArgs) -> Result<Vec<PathRow>> {
    let lake = Lake::load(ctx)?;
    let at = lake.resolve(&args.path, true)?;
    let listing = lake.list(&at, args.recursive)?;
    let in_window = |when: Option<time::OffsetDateTime>| {
        args.since
            .is_none_or(|since| when.is_some_and(|when| when >= since.0))
            && args
                .until
                .is_none_or(|until| when.is_some_and(|when| when < until.0))
    };
    let kept: Vec<_> = listing
        .entries
        .into_iter()
        .filter(|entry| in_window(entry.modified))
        .collect();
    let folders = kept.iter().filter(|entry| entry.directory).count();
    let total: u64 = kept.iter().map(|entry| entry.size).sum();
    let mut note = format!(
        "[{} files, {} under {}",
        kept.len() - folders,
        bytes(total),
        at.id()
    );
    if folders > 0 {
        note = note.replacen('[', &format!("[{folders} folders, "), 1);
    }
    if !listing.complete {
        note.push_str(", read until the deadline; --timeout S for all");
    }
    if kept.len() > args.limit {
        note.push_str(&format!("; {} of {}; --limit N", args.limit, kept.len()));
    }
    ctx.note(format!("{note}]"));
    Ok(kept
        .iter()
        .take(args.limit)
        .map(|entry| PathRow {
            id: at.at(&entry.path).id(),
            kind: if entry.directory { "directory" } else { "file" },
            size: entry.size,
            modified: stamp(entry),
            owner: entry.owner.clone(),
        })
        .collect())
}

command! {
    pub PATH_LIST = ["lake", "path", "list"], Read,
    "List the files and folders under a Data Lake folder, with their total size",
    keywords: ["ls", "files", "folder", "directory", "landed", "land", "arrived", "today", "browse", "abfss"],
    example: "lake path list contosolake/raw/sales/orders --since 1d --fields id,size,modified",
    run: path_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;

    use crate::LAKE;
    use crate::lake::testing::{dir, file, file_at, listing};
    use crate::testing::{self, azure};

    #[test]
    fn a_folder_lists_one_level_filtered_by_time_with_the_totals_noted() {
        let (outcome, transport) = azure(
            &[LAKE],
            &[
                "lake",
                "path",
                "list",
                "abfss://raw@contosolake.dfs.core.windows.net/sales/orders",
                "--since",
                "2026-09-29",
                "--limit",
                "1",
            ],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                listing(vec![
                    dir("sales/orders/date=2026-09-29"),
                    file_at(
                        "sales/orders/old.parquet",
                        10,
                        "Thu, 25 Sep 2026 06:02:11 GMT",
                    ),
                ])
                .with_header("x-ms-continuation", "page 2"),
                listing(vec![file("sales/orders/new.parquet", 3_000_000)]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got[0]["id"], "contosolake/raw/sales/orders/date=2026-09-29");
        assert_eq!(got[0]["kind"], "directory");
        assert_eq!(got.as_array().unwrap().len(), 1);
        assert!(
            outcome.stderr.contains("[1 folders, 1 files, 2.9 MiB under contosolake/raw/sales/orders; 1 of 2; --limit N]"),
            "{}",
            outcome.stderr
        );
        let sent = transport.sent();
        assert_eq!(
            sent[1].url,
            "https://contosolake.dfs.core.windows.net/raw?resource=filesystem&recursive=false&maxResults=5000&directory=sales%2Forders"
        );
        assert!(
            sent[2].url.ends_with("&continuation=page%202"),
            "{}",
            sent[2].url
        );
    }

    #[test]
    fn a_missing_folder_is_exit_4_naming_its_parent() {
        let (outcome, _) = azure(
            &[LAKE],
            &[
                "lake",
                "path",
                "list",
                "contosolake/raw/sales/order",
                "--recursive",
            ],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                Answer::status(
                    404,
                    r#"{"error":{"code":"PathNotFound","message":"The specified path does not exist."}}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli lake path list contosolake/raw/sales"),
            "{}",
            outcome.stderr
        );
    }
}
