//! `lake partition list`: a table's partitions, newest first, each with its
//! files, size, rows and last write, from one recursive listing and the
//! footers of the shown partitions' newest files. A date-valued partition
//! column's missing days go in a note.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Failure, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};
use time::OffsetDateTime;

use crate::lake::table::{FOOTERS, bulk, day, files, partitions, read_footers};
use crate::lake::{Entry, Lake};

/// The most missing days a note names.
const GAPS: usize = 10;

#[derive(clap::Args)]
pub struct PartitionListArgs {
    /// The table's folder: an id from lake table list, or its abfss:// or https:// URL
    path: String,
    /// Only partitions written to since then
    #[arg(long)]
    since: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PartitionRow {
    /// The partition's folder, which `lake path list` and `lake query run --table` take.
    id: String,
    /// Each partition column's value: `{"date": "2026-09-29"}`.
    values: Map<String, Value>,
    files: usize,
    size: u64,
    /// Rows, when every file's footer was read.
    rows: Option<u64>,
    /// When its newest file was written.
    modified: Option<String>,
    /// What Synapse's `OPENROWSET(BULK …)` takes for its files (`sql query run`).
    bulk: String,
}

struct Partition<'f> {
    path: String,
    values: Vec<(&'f str, &'f str)>,
    files: Vec<&'f Entry>,
    modified: Option<OffsetDateTime>,
}

fn partition_list(ctx: &Ctx, args: PartitionListArgs) -> Result<Vec<PartitionRow>> {
    let lake = Lake::load(ctx)?;
    let table = lake.resolve(&args.path, true)?;
    let found = files(&lake, &table)?;
    let mut grouped: BTreeMap<String, Partition> = BTreeMap::new();
    for file in &found {
        let values = partitions(&table, file);
        if values.is_empty() {
            continue;
        }
        let relative = file.path.strip_prefix(&table.path).unwrap_or(&file.path);
        let folders: Vec<&str> = relative
            .trim_start_matches('/')
            .split('/')
            .take(values.len())
            .collect();
        let path = table
            .at(&format!("{}/{}", table.path, folders.join("/")))
            .path;
        let held = grouped.entry(path.clone()).or_insert_with(|| Partition {
            path,
            values: values.clone(),
            files: Vec::new(),
            modified: None,
        });
        held.files.push(file);
        held.modified = held.modified.max(file.modified);
    }
    if grouped.is_empty() {
        return Err(Failure::not_found(format!(
            "{} has no partition folders (key=value)",
            table.id()
        ))
        .hint(format!(
            "`agent-cli lake table get {}` reads an unpartitioned table",
            table.id()
        ))
        .into());
    }
    if let Some(gap) = gaps(grouped.values()) {
        ctx.note(gap);
    }
    let mut shown: Vec<Partition> = grouped
        .into_values()
        .rev()
        .filter(|held| {
            args.since
                .is_none_or(|since| held.modified.is_some_and(|at| at >= since.0))
        })
        .collect();
    if shown.len() > args.limit {
        ctx.note(format!(
            "[{} of {} partitions; --limit N]",
            args.limit,
            shown.len()
        ));
        shown.truncate(args.limit);
    }
    let mut newest: Vec<Entry> = shown
        .iter()
        .flat_map(|held| held.files.iter().map(|file| (*file).clone()))
        .collect();
    newest.sort_by_key(|file| std::cmp::Reverse(file.modified));
    let read = read_footers(ctx, &lake, &table, &newest)?;
    if newest.len() > FOOTERS {
        ctx.note(format!(
            "[rows only where all a partition's files are among the newest {FOOTERS}]"
        ));
    }
    Ok(shown
        .into_iter()
        .map(|held| {
            let rows: Option<u64> = held
                .files
                .iter()
                .map(|file| {
                    read.iter()
                        .find(|(entry, _)| entry.path == file.path)
                        .map(|(_, footer)| footer.rows)
                })
                .sum();
            let folder = table.at(&held.path);
            PartitionRow {
                id: folder.id(),
                bulk: bulk(&folder, &[]),
                values: held
                    .values
                    .iter()
                    .map(|(key, value)| {
                        (
                            (*key).to_owned(),
                            Value::String(crate::client::decode(value)),
                        )
                    })
                    .collect(),
                files: held.files.len(),
                size: held.files.iter().map(|file| file.size).sum(),
                rows,
                modified: held.modified.map(agent_cli_core::utc_time),
            }
        })
        .collect())
}

/// The missing days of the first partition column whose values are all
/// days, between its first and last.
fn gaps<'p>(partitions: impl Iterator<Item = &'p Partition<'p>> + Clone) -> Option<String> {
    let first = partitions.clone().next()?;
    let (column, at) = first.values.iter().enumerate().find_map(|(at, (key, _))| {
        partitions
            .clone()
            .all(|held| {
                held.values
                    .get(at)
                    .is_some_and(|(_, value)| day(value).is_some())
            })
            .then_some((*key, at))
    })?;
    let days: std::collections::BTreeSet<time::Date> = partitions
        .filter_map(|held| day(held.values[at].1))
        .collect();
    let (start, end) = (*days.first()?, *days.last()?);
    let mut missing = Vec::new();
    let mut at = start;
    while at < end {
        if !days.contains(&at) {
            missing.push(format!("{column}={at}"));
        }
        at = at.next_day()?;
    }
    if missing.is_empty() {
        return None;
    }
    let more = missing.len().saturating_sub(GAPS);
    missing.truncate(GAPS);
    let more = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    Some(format!(
        "[missing between {start} and {end}: {}{more}]",
        missing.join(", ")
    ))
}

command! {
    pub PARTITION_LIST = ["lake", "partition", "list"], Read,
    "List a Parquet table's partitions with files, rows and writes; note missing days",
    keywords: ["missing", "gaps", "late", "date", "daily", "hive"],
    example: "lake partition list contosolake/raw/sales/orders --since 7d --fields id,files,rows,modified",
    run: partition_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::LAKE;
    use crate::lake::testing::{file, file_at, listing, tail, world};
    use crate::testing::{self, azure};

    #[test]
    fn partitions_come_newest_first_with_rows_and_a_missing_day_noted() {
        let one = world("contosolake/raw/sales/orders/date=2026-09-28/part-0001.parquet");
        let (outcome, _) = azure(
            &[LAKE],
            &["lake", "partition", "list", "contosolake/raw/sales/orders"],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                listing(vec![
                    file_at(
                        "sales/orders/date=2026-09-25/part-0001.parquet",
                        one.len(),
                        "Thu, 25 Sep 2026 06:01:00 GMT",
                    ),
                    file_at(
                        "sales/orders/date=2026-09-26/part-0001.parquet",
                        one.len(),
                        "Fri, 26 Sep 2026 06:01:00 GMT",
                    ),
                    file("sales/orders/date=2026-09-29/part-0001.parquet", one.len()),
                ]),
                tail(&one),
                tail(&one),
                tail(&one),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got[0],
            json!({"id": "contosolake/raw/sales/orders/date=2026-09-29", "values": {"date": "2026-09-29"},
                "files": 1, "size": one.len(), "rows": 4, "modified": "2026-09-29T06:02:11Z",
                "bulk": "https://contosolake.dfs.core.windows.net/raw/sales/orders/date=2026-09-29/*.parquet"})
        );
        assert_eq!(got[2]["id"], "contosolake/raw/sales/orders/date=2026-09-25");
        assert!(
            outcome.stderr.contains(
                "[missing between 2026-09-25 and 2026-09-29: date=2026-09-27, date=2026-09-28]"
            ),
            "{}",
            outcome.stderr
        );
    }
}
