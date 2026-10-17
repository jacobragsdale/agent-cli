//! `lake table get`: a table's schema, files, size, rows and last write,
//! from one recursive listing and the footers of its newest files.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::lake::Lake;
use crate::lake::parquet::Column;
use crate::lake::table::{FOOTERS, files, newest, partitioned_by, read_footers};

#[derive(clap::Args)]
pub struct TableGetArgs {
    /// The table's folder: an id from lake table list, or its abfss:// or https:// URL
    path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TableDetail {
    /// The table's folder.
    id: String,
    partitioned_by: Vec<String>,
    /// The newest file's columns; a note names files whose columns differ.
    schema: Vec<Column>,
    files: usize,
    size: u64,
    /// Rows in the files whose footers were read: every file, up to 200.
    rows: u64,
    /// When the newest file was written.
    modified: Option<String>,
    /// The newest file, which `lake path get` takes.
    newest: Option<String>,
}

fn table_get(ctx: &Ctx, args: TableGetArgs) -> Result<TableDetail> {
    let lake = Lake::load(ctx)?;
    let table = lake.resolve(&args.path, true)?;
    let found = files(&lake, &table)?;
    if found.is_empty() {
        return Err(
            Failure::not_found(format!("no Parquet files under {}", table.id()))
                .hint(format!(
                    "`agent-cli lake table list {}` finds the tables there",
                    table.id()
                ))
                .into(),
        );
    }
    let read = read_footers(ctx, &lake, &table, &found)?;
    let schema = read
        .first()
        .map(|(_, footer)| footer.schema.clone())
        .unwrap_or_default();
    let differ: Vec<String> = read
        .iter()
        .filter(|(_, footer)| footer.schema != schema)
        .map(|(file, _)| table.at(&file.path).id())
        .collect();
    if !differ.is_empty() {
        let more = if differ.len() > 3 {
            format!(" and {} more", differ.len() - 3)
        } else {
            String::new()
        };
        ctx.note(format!(
            "[columns differ from the newest file's in {}{more}]",
            differ
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if read.len() < found.len() {
        ctx.note(format!(
            "[rows counts the newest {} of {} files; footers are read for at most {FOOTERS}]",
            read.len(),
            found.len()
        ));
    }
    Ok(TableDetail {
        id: table.id(),
        partitioned_by: partitioned_by(&table, &found),
        schema,
        files: found.len(),
        size: found.iter().map(|file| file.size).sum(),
        rows: read.iter().map(|(_, footer)| footer.rows).sum(),
        modified: newest(&found),
        newest: found.first().map(|file| table.at(&file.path).id()),
    })
}

command! {
    pub TABLE_GET = ["lake", "table", "get"], Read,
    "Show a Parquet table's schema, rows, size and last write, read from its footers",
    keywords: ["columns", "count", "rows", "metadata", "dataset", "last", "written", "updated"],
    example: "lake table get contosolake/curated/finance/ledger --fields schema,rows,modified",
    run: table_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::LAKE;
    use crate::lake::testing::{file, file_at, listing, parquet, tail};
    use crate::testing::{self, azure};

    #[test]
    fn the_schema_is_the_newest_files_and_rows_add_up_across_footers() {
        let new = parquet("SELECT range AS id, 1.5::DOUBLE AS amount FROM range(3)");
        let old = parquet("SELECT range AS id FROM range(2)");
        let (outcome, _) = azure(
            &[LAKE],
            &["lake", "table", "get", "contosolake/curated/finance/ledger"],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                listing(vec![
                    file_at(
                        "finance/ledger/part-0.parquet",
                        old.len(),
                        "Sun, 28 Sep 2026 06:00:00 GMT",
                    ),
                    file("finance/ledger/part-1.parquet", new.len()),
                ]),
                tail(&new),
                tail(&old),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            (got["rows"].clone(), got["files"].clone()),
            (json!(5), json!(2))
        );
        assert_eq!(
            got["newest"],
            "contosolake/curated/finance/ledger/part-1.parquet"
        );
        assert_eq!(
            got["schema"][1],
            json!({"name": "amount", "type": "DOUBLE"})
        );
        assert!(
            outcome.stderr.contains("[columns differ from the newest file's in contosolake/curated/finance/ledger/part-0.parquet]"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_folder_without_parquet_files_is_exit_4_naming_table_list() {
        let (outcome, _) = azure(
            &[LAKE],
            &["lake", "table", "get", "contosolake/raw/sales"],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                listing(vec![]),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli lake table list contosolake/raw/sales"),
            "{}",
            outcome.stderr
        );
    }
}
