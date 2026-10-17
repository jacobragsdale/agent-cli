//! Parquet without downloading it: a file ends in its metadata, the
//! metadata's length and `PAR1`, so a ranged GET of the tail fetches what
//! DuckDB's `parquet_file_metadata` and `DESCRIBE` read. The tail is written
//! at its offset into a sparse file of the file's size, which DuckDB reads
//! without touching the holes. `query run` downloads whole files instead
//! (`download`), into a folder it reuses while their etags hold.
//!
//! DuckDB runs in memory with extensions neither installed nor loaded, and
//! may read no file outside the one folder it is handed ([`duck`]).

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use agent_cli_core::Failure;
use anyhow::{Context, Result, bail};
use duckdb::Connection;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::parallel;
use crate::lake::{Lake, Ref};

/// One tail read holds the footer of all but the widest files.
const TAIL: u64 = 64 * 1024;
/// A download's ranged reads, each well under core's 64 MiB answer cap.
const CHUNK: u64 = 32 * 1024 * 1024;

/// A column as DuckDB reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Column {
    pub name: String,
    /// DuckDB's type: `BIGINT`, `VARCHAR`, `DATE`, `STRUCT(…)`.
    #[serde(rename = "type")]
    pub kind: String,
}

/// What a Parquet footer says.
#[derive(Clone, Debug)]
pub(crate) struct Footer {
    pub rows: u64,
    /// The library that wrote it (`created_by`).
    pub writer: Option<String>,
    pub schema: Vec<Column>,
}

/// An in-memory DuckDB that loads no extension and reads only under `dir`.
pub(crate) fn duck(dir: &Path) -> Result<Connection> {
    let config = duckdb::Config::default()
        .enable_autoload_extension(false)?
        .with("autoinstall_known_extensions", "false")?;
    let connection = Connection::open_in_memory_with_flags(config)?;
    let dir = format!("{}/", dir.display());
    connection.execute_batch(&format!(
        "SET allowed_directories = [{}]; SET enable_external_access = false; SET lock_configuration = true;",
        literal(&dir)
    ))?;
    Ok(connection)
}

/// `raw` as a SQL string literal.
pub(crate) fn literal(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "''"))
}

/// Each file's footer (`files`: each one's id and size), read side by side;
/// one that cannot be read says why.
pub(crate) fn footers(lake: &Lake, files: &[(Ref, u64)]) -> Result<Vec<Result<Footer>>> {
    let dir = tempfile::tempdir()?;
    let indexed: Vec<(usize, &(Ref, u64))> = files.iter().enumerate().collect();
    let tails = parallel(&indexed, lake.azure.parallel(), |(index, (file, size))| {
        let local = dir.path().join(format!("{index}.parquet"));
        tail(lake, file, *size, &local).map(|()| local)
    });
    let connection = duck(dir.path())?;
    Ok(tails
        .into_iter()
        .map(|local| local.and_then(|local| describe(&connection, &local)))
        .collect())
}

/// The file's footer, written at its offset into `local`, a sparse file of
/// the same size that also starts with `PAR1`.
fn tail(lake: &Lake, file: &Ref, size: u64, local: &Path) -> Result<()> {
    let not_parquet = |why: &str| anyhow::anyhow!("{} is not a Parquet file: {why}", file.id());
    if size < 12 {
        return Err(not_parquet("too small"));
    }
    let url = file.url();
    let want = size.min(TAIL);
    let mut held = lake.range(&url, size - want, size - 1)?;
    if held.len() as u64 != want || &held[held.len() - 4..] != b"PAR1" {
        return Err(not_parquet("no PAR1 at its end"));
    }
    let length = u64::from(u32::from_le_bytes(
        held[held.len() - 8..held.len() - 4].try_into()?,
    ));
    let need = length + 8;
    if need + 4 > size {
        return Err(not_parquet("its footer is longer than the file"));
    }
    if need > want {
        let mut more = lake.range(&url, size - need, size - want - 1)?;
        more.extend_from_slice(&held);
        held = more;
    }
    let mut out = File::create(local)?;
    out.set_len(size)?;
    out.write_all(b"PAR1")?;
    out.seek(SeekFrom::Start(size - held.len() as u64))?;
    out.write_all(&held)?;
    Ok(())
}

/// The rows, writer and schema of the Parquet file at `local`.
pub(crate) fn describe(connection: &Connection, local: &Path) -> Result<Footer> {
    let path = literal(&local.display().to_string());
    let (rows, writer) = connection
        .query_row(
            &format!("SELECT sum(num_rows)::UBIGINT, any_value(created_by) FROM parquet_file_metadata({path})"),
            [],
            |row| Ok((row.get::<_, Option<u64>>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .context("reading its Parquet footer")?;
    let mut statement = connection.prepare(&format!(
        "DESCRIBE SELECT * FROM read_parquet({path}, hive_partitioning = false)"
    ))?;
    let schema = statement
        .query_map([], |row| {
            Ok(Column {
                name: row.get(0)?,
                kind: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Footer {
        rows: rows.unwrap_or(0),
        writer,
        schema,
    })
}

/// One file to download: its id, size and etag.
#[derive(Clone, Debug)]
pub(crate) struct Wanted {
    pub file: Ref,
    pub size: u64,
    pub etag: String,
}

/// Each file under `root`, at `account/container/path` so its partition
/// folders stay in its path, its etag in its name: one already there is not
/// fetched again. Each local path in `files`' order, and whether it was
/// already there.
pub(crate) fn download(lake: &Lake, root: &Path, files: &[Wanted]) -> Result<Vec<(PathBuf, bool)>> {
    parallel(
        files,
        lake.azure.parallel(),
        |wanted| -> Result<(PathBuf, bool)> {
            let local = local_path(root, wanted);
            if local.metadata().is_ok_and(|held| held.len() == wanted.size) {
                return Ok((local, true));
            }
            let folder = local.parent().context("a download has no folder")?;
            std::fs::create_dir_all(folder)?;
            // A temp file renamed into place: a half download is never taken
            // for the file.
            let mut out = tempfile::NamedTempFile::new_in(folder)?;
            let url = wanted.file.url();
            let mut at = 0;
            while at < wanted.size {
                let end = (at + CHUNK).min(wanted.size) - 1;
                let chunk = lake.range(&url, at, end)?;
                if chunk.len() as u64 != end - at + 1 {
                    bail!(
                        "{} answered {} bytes for {at}-{end}; it changed while downloading",
                        wanted.file.id(),
                        chunk.len()
                    );
                }
                out.write_all(&chunk)?;
                at = end + 1;
            }
            out.persist(&local).map_err(|error| error.error)?;
            Ok((local, false))
        },
    )
    .into_iter()
    .collect()
}

fn local_path(root: &Path, wanted: &Wanted) -> PathBuf {
    let tag: String = wanted
        .etag
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let name = wanted.file.name();
    let stem = name.strip_suffix(".parquet").unwrap_or(name);
    let mut local = root.join(&wanted.file.account).join(&wanted.file.container);
    if let Some(parent) = wanted.file.parent() {
        for segment in parent.path.split('/').filter(|segment| !segment.is_empty()) {
            // `..` from a listing must not climb out of the folder.
            local.push(if segment == ".." { "%2E%2E" } else { segment });
        }
    }
    local.join(format!("{stem}.{tag}.parquet"))
}

/// The folder downloads go to: under the cache, kept between runs and
/// private to the user, or a temporary one when the cache is off.
pub(crate) fn downloads(lake: &Lake) -> Result<(PathBuf, Option<tempfile::TempDir>)> {
    let Some(cache) = lake.ctx().cache().dir() else {
        let held = tempfile::tempdir()?;
        return Ok((held.path().to_owned(), Some(held)));
    };
    let root = cache.join("lake");
    std::fs::create_dir_all(&root)
        .map_err(|error| Failure::setup(format!("cannot make {}: {error}", root.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok((root, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_footer_alone_in_a_sparse_file_gives_the_rows_writer_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        let whole = dir.path().join("whole.parquet");
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(&format!(
                "COPY (SELECT range AS id, 'x' || range AS name FROM range(1000)) TO {} (FORMAT parquet)",
                literal(&whole.display().to_string())
            ))
            .unwrap();
        let bytes = std::fs::read(&whole).unwrap();
        let size = bytes.len() as u64;
        let length = u64::from(u32::from_le_bytes(
            bytes[bytes.len() - 8..bytes.len() - 4].try_into().unwrap(),
        ));
        let tail = &bytes[bytes.len() - (length as usize + 8)..];

        let sparse = dir.path().join("sparse.parquet");
        let mut out = File::create(&sparse).unwrap();
        out.set_len(size).unwrap();
        out.write_all(b"PAR1").unwrap();
        out.seek(SeekFrom::Start(size - tail.len() as u64)).unwrap();
        out.write_all(tail).unwrap();
        drop(out);

        let footer = describe(&duck(dir.path()).unwrap(), &sparse).unwrap();
        assert_eq!(footer.rows, 1000);
        assert!(footer.writer.unwrap().starts_with("DuckDB"));
        assert_eq!(
            footer.schema,
            [
                Column {
                    name: "id".into(),
                    kind: "BIGINT".into()
                },
                Column {
                    name: "name".into(),
                    kind: "VARCHAR".into()
                },
            ]
        );
    }

    #[test]
    fn duckdb_reads_nothing_outside_its_folder_and_loads_no_extension() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("o.parquet");
        Connection::open_in_memory()
            .unwrap()
            .execute_batch(&format!(
                "COPY (SELECT 1 AS a) TO {} (FORMAT parquet)",
                literal(&file.display().to_string())
            ))
            .unwrap();
        let connection = duck(dir.path()).unwrap();
        for sql in [
            format!(
                "SELECT * FROM read_parquet({})",
                literal(&file.display().to_string())
            ),
            "INSTALL httpfs".to_owned(),
            "SET enable_external_access = true".to_owned(),
        ] {
            assert!(connection.execute_batch(&sql).is_err(), "{sql}");
        }
    }

    #[test]
    fn a_download_keeps_its_partition_folders_and_never_climbs_out() {
        let wanted = |path: &str| Wanted {
            file: Ref::parse(path).unwrap(),
            size: 1,
            etag: "\"0x8DD1\"".into(),
        };
        let root = Path::new("/cache/lake");
        assert_eq!(
            local_path(
                root,
                &wanted("contosolake/raw/sales/orders/date=2026-09-29/part-0001.parquet")
            ),
            Path::new(
                "/cache/lake/contosolake/raw/sales/orders/date=2026-09-29/part-0001.0x8DD1.parquet"
            )
        );
        assert_eq!(
            local_path(root, &wanted("contosolake/raw/a/../../b.parquet")),
            Path::new("/cache/lake/contosolake/raw/a/%2E%2E/%2E%2E/b.0x8DD1.parquet")
        );
    }
}
