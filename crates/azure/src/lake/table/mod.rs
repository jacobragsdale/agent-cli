//! Tables in a listing, which `table list`, `table get` and `partition list`
//! share. A table is a folder holding `.parquet` files or Hive partition
//! folders (`date=2026-09-29`); folders whose names start with `_` or `.`
//! (`_temporary`, `.spark-staging`) and the files in them are left out, as
//! Spark and DuckDB leave them out.

pub(crate) mod get;
pub(crate) mod list;

use agent_cli_core::{Ctx, utc_time};
use anyhow::Result;

use crate::lake::parquet::{Footer, footers};
use crate::lake::{Entry, Lake, Ref};

/// The most files whose footers a command reads.
pub(crate) const FOOTERS: usize = 200;

/// A Hive partition folder's key and value.
pub(crate) fn partition(segment: &str) -> Option<(&str, &str)> {
    segment
        .split_once('=')
        .filter(|(key, _)| !key.is_empty() && !hidden(key))
}

pub(crate) fn hidden(segment: &str) -> bool {
    segment.starts_with('_') || segment.starts_with('.')
}

/// Whether `path` is a Parquet file outside every hidden folder.
pub(crate) fn parquet(entry: &Entry) -> bool {
    !entry.directory && entry.path.ends_with(".parquet") && !entry.path.split('/').any(hidden)
}

/// Whether a folder's own entries make it a table.
pub(crate) fn is_table(children: &[Entry]) -> bool {
    children.iter().any(|child| {
        let name = child.path.rsplit('/').next().unwrap_or_default();
        if child.directory {
            partition(name).is_some()
        } else {
            parquet(child)
        }
    })
}

/// The table's Parquet files, newest first, read in one recursive listing;
/// a note when the deadline cut it short.
pub(crate) fn files(lake: &Lake, root: &Ref) -> Result<Vec<Entry>> {
    let listing = lake.list(root, true)?;
    if !listing.complete {
        lake.ctx().note(format!(
            "[the listing of {} stopped at the deadline after {} entries; --timeout S for all]",
            root.id(),
            listing.entries.len()
        ));
    }
    let mut files: Vec<Entry> = listing.entries.into_iter().filter(parquet).collect();
    files.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(files)
}

/// The partition folders between `root` and `file`, outermost first.
pub(crate) fn partitions<'f>(root: &Ref, file: &'f Entry) -> Vec<(&'f str, &'f str)> {
    let relative = file
        .path
        .strip_prefix(&root.path)
        .unwrap_or(&file.path)
        .trim_start_matches('/');
    let mut found = Vec::new();
    let mut segments: Vec<&str> = relative.split('/').collect();
    segments.pop();
    for segment in segments {
        match partition(segment) {
            Some(pair) => found.push(pair),
            None => break,
        }
    }
    found
}

/// The partition columns, from the newest file.
pub(crate) fn partitioned_by(root: &Ref, files: &[Entry]) -> Vec<String> {
    files.first().map_or_else(Vec::new, |file| {
        partitions(root, file)
            .into_iter()
            .map(|(key, _)| key.to_owned())
            .collect()
    })
}

/// The newest change among `files`, as printed.
pub(crate) fn newest(files: &[Entry]) -> Option<String> {
    files
        .iter()
        .filter_map(|file| file.modified)
        .max()
        .map(utc_time)
}

/// The footers of `files` (at most [`FOOTERS`], the newest), each paired
/// with its entry; a footer that cannot be read is named in a note.
pub(crate) fn read_footers<'f>(
    ctx: &Ctx,
    lake: &Lake,
    root: &Ref,
    files: &'f [Entry],
) -> Result<Vec<(&'f Entry, Footer)>> {
    let read: Vec<&Entry> = files.iter().take(FOOTERS).collect();
    let wanted: Vec<(Ref, u64)> = read
        .iter()
        .map(|file| (root.at(&file.path), file.size))
        .collect();
    let mut found = Vec::new();
    let mut failed = Vec::new();
    for (file, footer) in read.into_iter().zip(footers(lake, &wanted)?) {
        match footer {
            Ok(footer) => found.push((file, footer)),
            Err(error) => failed.push(format!("{error:#}")),
        }
    }
    if let Some(first) = failed.first() {
        ctx.note(format!(
            "[{} footers not read; the first: {first}]",
            failed.len()
        ));
    }
    Ok(found)
}

/// When `entry` last changed, as printed.
pub(crate) fn stamp(entry: &Entry) -> Option<String> {
    entry.modified.map(utc_time)
}

/// A day, when `value` is one (`2026-09-29`).
pub(crate) fn day(value: &str) -> Option<time::Date> {
    time::Date::parse(
        value,
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, directory: bool) -> Entry {
        Entry {
            path: path.into(),
            directory,
            ..Entry::default()
        }
    }

    #[test]
    fn a_table_is_a_folder_of_parquet_files_or_partition_folders_and_hidden_ones_do_not_count() {
        assert!(is_table(&[entry("sales/orders/date=2026-09-29", true)]));
        assert!(is_table(&[entry("sales/daily/part-0.parquet", false)]));
        assert!(!is_table(&[
            entry("sales/orders", true),
            entry("sales/_SUCCESS", false),
            entry("sales/_temporary/part-0.parquet", false),
            entry("sales/_k=v", true),
        ]));
        let root = Ref::parse("contosolake/raw/sales/orders").unwrap();
        let file = entry("sales/orders/date=2026-09-29/hour=06/part-0.parquet", false);
        assert_eq!(
            partitions(&root, &file),
            [("date", "2026-09-29"), ("hour", "06")]
        );
        assert_eq!(partitioned_by(&root, &[file]), ["date", "hour"]);
    }
}
