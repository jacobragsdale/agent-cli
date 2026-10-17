//! `lake query run`: SQL in an in-memory DuckDB over Parquet files from the
//! lake. Each `--table NAME=REF` names its files (one file, a folder's, or a
//! glob in the last segment), so nothing parses the SQL and every byte
//! fetched is on the command line. The files download through `Ctx` into a
//! folder kept by etag (`parquet::download`), past `--max-mb` nothing
//! downloads, and the view reads them with Hive partitioning, so partition
//! folders' values are columns. A Read whatever the SQL says: the database
//! is in memory and may read nothing outside that folder.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use agent_cli_core::{Ctx, Failure, command};
use anyhow::{Context, Result};
use duckdb::types::Value as Duck;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::lake::parquet::{Wanted, download, downloads, duck, literal};
use crate::lake::table::{hidden, parquet};
use crate::lake::{Entry, Lake, Ref, bytes};

#[derive(clap::Args)]
pub struct QueryRunArgs {
    /// The SQL (DuckDB's dialect), or - to read it from stdin. Statements before the last run; the last one's rows return
    #[arg(allow_hyphen_values = true, required_unless_present = "sql_file")]
    sql: Option<String>,
    /// The SQL from a file
    #[arg(long)]
    sql_file: Option<PathBuf>,
    /// A view the SQL reads: NAME=REF, REF a Parquet file, a folder (every file under it) or a glob in its last segment (…/date=2026-09-2*); repeat for more
    #[arg(long, required = true)]
    table: Vec<String>,
    /// The most to download, all tables together, in MiB
    #[arg(long, default_value_t = 1024)]
    max_mb: u64,
    /// Keep at most this many rows
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u64).range(1..))]
    max_rows: u64,
}

/// As `sql query run` answers.
#[derive(Debug, Serialize, JsonSchema)]
pub struct QueryResult {
    /// The last statement's rows.
    results: Vec<ResultSet>,
    elapsed_ms: u64,
    read: Read,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ResultSet {
    columns: Vec<String>,
    /// DuckDB's types: `BIGINT`, `VARCHAR`, `DATE`.
    types: Vec<String>,
    /// Positional, in `columns` order. Integers past 2^53 and decimals are
    /// strings; dates and times RFC 3339, `Z` for a TIMESTAMPTZ.
    rows: Vec<Vec<Value>>,
    /// There were more than `--max-rows`.
    truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Read {
    files: usize,
    /// Bytes downloaded this run.
    bytes: u64,
    /// Files already downloaded by an earlier run, unchanged since.
    cached: usize,
}

/// `NAME=REF`, NAME an identifier.
fn binding(raw: &str) -> Result<(String, String)> {
    let usage = || {
        Failure::usage(format!("--table takes NAME=REF, not {raw:?}"))
        .hint("agent-cli lake query run 'select count(*) from orders' --table orders=contosolake/raw/sales/orders")
    };
    let (name, at) = raw.split_once('=').ok_or_else(usage)?;
    let name = name.trim();
    let ident = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && name
            .chars()
            .all(|held| held.is_ascii_alphanumeric() || held == '_');
    if !ident || at.trim().is_empty() {
        return Err(usage().into());
    }
    Ok((name.to_owned(), at.trim().to_owned()))
}

/// Whether `name` matches `pattern`'s `*` and `?`.
fn glob(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.first(), name.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob(&pattern[1..], name) || (!name.is_empty() && glob(pattern, &name[1..]))
        }
        (Some(b'?'), Some(_)) => glob(&pattern[1..], &name[1..]),
        (Some(wanted), Some(held)) if wanted == held => glob(&pattern[1..], &name[1..]),
        _ => false,
    }
}

/// The Parquet files `at` names.
fn files(lake: &Lake, at: &Ref) -> Result<Vec<(Ref, Entry)>> {
    let name = at.name();
    if name.contains(['*', '?']) && !at.path.is_empty() {
        let parent = at.parent().unwrap_or_else(|| at.at(""));
        let mut found = Vec::new();
        for entry in lake.list(&parent, false)?.entries {
            let leaf = entry.path.rsplit('/').next().unwrap_or_default();
            if hidden(leaf) || !glob(name.as_bytes(), leaf.as_bytes()) {
                continue;
            }
            if entry.directory {
                found.extend(under(lake, &at.at(&entry.path))?);
            } else if parquet(&entry) {
                found.push((at.at(&entry.path), entry));
            }
        }
        return Ok(found);
    }
    let properties = lake.head(&at.head_url())?;
    if !at.path.is_empty() && properties.header("x-ms-resource-type") == Some("file") {
        let entry = Entry {
            path: at.path.clone(),
            size: properties
                .header("content-length")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(0),
            etag: properties.header("etag").map(str::to_owned),
            ..Entry::default()
        };
        return Ok(vec![(at.clone(), entry)]);
    }
    under(lake, at)
}

fn under(lake: &Lake, folder: &Ref) -> Result<Vec<(Ref, Entry)>> {
    let listing = lake.list(folder, true)?;
    if !listing.complete {
        return Err(
            Failure::timed_out(format!("listing {} took the whole deadline", folder.id()))
                .hint("narrow --table to a partition, or run it again with a larger --timeout")
                .into(),
        );
    }
    Ok(listing
        .entries
        .into_iter()
        .filter(parquet)
        .map(|entry| (folder.at(&entry.path), entry))
        .collect())
}

fn query_run(ctx: &Ctx, args: QueryRunArgs) -> Result<QueryResult> {
    let started = Instant::now();
    let sql = ctx
        .long_text("sql", args.sql.as_deref(), args.sql_file.as_deref(), None)?
        .map(|sql| sql.text)
        .unwrap_or_default();
    let bound: Vec<(String, String)> = args
        .table
        .iter()
        .map(|raw| binding(raw))
        .collect::<Result<_>>()?;
    let lake = Lake::load(ctx)?;
    let mut tables: Vec<(String, Ref, Vec<Wanted>)> = Vec::new();
    for (name, raw) in &bound {
        let at = lake.resolve(raw, true)?;
        let found = files(&lake, &at)?;
        if found.is_empty() {
            return Err(Failure::not_found(format!(
                "--table {name}: no Parquet files at {}",
                at.id()
            ))
            .hint(format!(
                "`agent-cli lake path list {}` shows what is there",
                at.parent().unwrap_or_else(|| at.clone()).id()
            ))
            .into());
        }
        let wanted = found
            .into_iter()
            .map(|(file, entry)| Wanted {
                file,
                size: entry.size,
                etag: entry.etag.unwrap_or_default(),
            })
            .collect();
        tables.push((name.clone(), at, wanted));
    }
    let total: u64 = tables
        .iter()
        .flat_map(|(_, _, wanted)| wanted)
        .map(|wanted| wanted.size)
        .sum();
    if total > args.max_mb.saturating_mul(1024 * 1024) {
        let (_, biggest, _) = tables
            .iter()
            .max_by_key(|(_, _, wanted)| wanted.iter().map(|held| held.size).sum::<u64>())
            .context("no tables")?;
        return Err(Failure::usage(format!(
            "the tables are {} in {} files, past --max-mb {}; nothing was downloaded",
            bytes(total),
            tables.iter().map(|(_, _, wanted)| wanted.len()).sum::<usize>(),
            args.max_mb
        ))
        .hint(format!(
            "`agent-cli lake partition list {}` picks partitions to bind instead, or raise --max-mb",
            biggest.id()
        ))
        .into());
    }
    let (root, _held) = downloads(&lake)?;
    let connection = duck(&root)?;
    let mut read = Read {
        files: 0,
        bytes: 0,
        cached: 0,
    };
    for (name, _, wanted) in &tables {
        let got = download(&lake, &root, wanted)?;
        read.files += got.len();
        for (held, (_, cached)) in wanted.iter().zip(&got) {
            if *cached {
                read.cached += 1;
            } else {
                read.bytes += held.size;
            }
        }
        let list: Vec<String> = got
            .iter()
            .map(|(path, _)| literal(&path.display().to_string()))
            .collect();
        connection
            .execute_batch(&format!(
                "CREATE VIEW \"{name}\" AS SELECT * FROM read_parquet([{}], hive_partitioning = true)",
                list.join(", ")
            ))
            .map_err(|error| Failure::usage(format!("--table {name}: {error}")))?;
    }
    let keep = usize::try_from(args.max_rows).unwrap_or(usize::MAX);
    let set = run(ctx, &connection, &sql, keep)?;
    if set.truncated {
        ctx.note(format!(
            "[stopped at --max-rows {}; raise it for more]",
            args.max_rows
        ));
    }
    Ok(QueryResult {
        results: vec![set],
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        read,
    })
}

/// Runs `sql`, interrupted at the deadline.
fn run(ctx: &Ctx, connection: &duckdb::Connection, sql: &str, keep: usize) -> Result<ResultSet> {
    let left = ctx.remaining()?;
    let interrupt = connection.interrupt_handle();
    let (done, watch) = mpsc::channel::<()>();
    let ran = std::thread::scope(|scope| {
        scope.spawn(move || {
            if watch.recv_timeout(left) == Err(mpsc::RecvTimeoutError::Timeout) {
                interrupt.interrupt();
            }
        });
        let ran = collect(connection, sql, keep);
        drop(done);
        ran
    });
    ran.map_err(|error| {
        if ctx.remaining().is_err() {
            Failure::timed_out("the query was still running at the --timeout deadline")
                .hint("bind fewer partitions, or run it again with a larger --timeout")
                .into()
        } else {
            anyhow::Error::new(Failure::usage(format!("DuckDB: {error}")))
        }
    })
}

fn collect(connection: &duckdb::Connection, sql: &str, keep: usize) -> duckdb::Result<ResultSet> {
    let mut statement = connection.prepare(sql)?;
    let mut rows = statement.query([])?;
    let (columns, types, zoned) = {
        let held = rows.as_ref().expect("a query's rows hold their statement");
        let count = held.column_count();
        let mut types = Vec::new();
        let mut zoned = Vec::new();
        for index in 0..count {
            let logical = held.column_logical_type(index);
            let id = logical.id();
            zoned.push(id == duckdb::core::LogicalTypeId::TimestampTZ);
            types.push(match id {
                duckdb::core::LogicalTypeId::Decimal => {
                    format!(
                        "DECIMAL({},{})",
                        logical.decimal_width(),
                        logical.decimal_scale()
                    )
                }
                other => format!("{other:?}").to_ascii_uppercase(),
            });
        }
        (held.column_names(), types, zoned)
    };
    let mut out = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next()? {
        if out.len() == keep {
            truncated = true;
            break;
        }
        let mut cells = Vec::new();
        for (index, zone) in zoned.iter().enumerate() {
            cells.push(json_of(row.get_ref(index)?.to_owned(), *zone));
        }
        out.push(cells);
    }
    Ok(ResultSet {
        columns,
        types,
        rows: out,
        truncated,
    })
}

/// A DuckDB value as JSON, as `sql query run` prints its values.
fn json_of(value: Duck, zoned: bool) -> Value {
    const EXACT: i128 = 1 << 53;
    let int = |held: i128| {
        if held.abs() <= EXACT {
            json!(held as i64)
        } else {
            json!(held.to_string())
        }
    };
    let float = |held: f64| {
        if held.is_finite() {
            json!(held)
        } else {
            json!(held.to_string())
        }
    };
    match value {
        Duck::Null => Value::Null,
        Duck::Boolean(held) => json!(held),
        Duck::TinyInt(held) => json!(held),
        Duck::SmallInt(held) => json!(held),
        Duck::Int(held) => json!(held),
        Duck::BigInt(held) => int(i128::from(held)),
        Duck::HugeInt(held) => int(held),
        Duck::UTinyInt(held) => json!(held),
        Duck::USmallInt(held) => json!(held),
        Duck::UInt(held) => json!(held),
        Duck::UBigInt(held) => int(i128::from(held)),
        Duck::UHugeInt(held) => i128::try_from(held).map_or_else(|_| json!(held.to_string()), int),
        Duck::Float(held) => float(f64::from(held)),
        Duck::Double(held) => float(held),
        Duck::Decimal(held) => json!(held.to_string()),
        Duck::Timestamp(unit, held) => {
            let instant =
                OffsetDateTime::from_unix_timestamp_nanos(i128::from(unit.to_micros(held)) * 1000);
            match instant.ok().and_then(|at| at.format(&Rfc3339).ok()) {
                Some(text) if zoned => json!(text),
                Some(text) => json!(text.trim_end_matches('Z')),
                None => json!(held.to_string()),
            }
        }
        Duck::Text(held) | Duck::Enum(held) => json!(held),
        Duck::Blob(held) | Duck::Geometry(held) => {
            json!(format!(
                "0x{}",
                held.iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ))
        }
        Duck::Date32(days) => time::Date::from_julian_day(2_440_588 + days)
            .map_or_else(|_| json!(days.to_string()), |day| json!(day.to_string())),
        Duck::Time64(unit, held) => {
            let micros = unit.to_micros(held);
            let midnight = time::Time::MIDNIGHT + time::Duration::microseconds(micros);
            json!(midnight.to_string())
        }
        Duck::Interval {
            months,
            days,
            nanos,
        } => {
            json!(format!("P{months}M{days}DT{}S", nanos as f64 / 1e9))
        }
        Duck::List(items) | Duck::Array(items) => {
            Value::Array(items.into_iter().map(|item| json_of(item, false)).collect())
        }
        Duck::Struct(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, held)| (name.clone(), json_of(held.clone(), false)))
                .collect(),
        ),
        Duck::Map(pairs) => Value::Array(
            pairs
                .iter()
                .map(|(key, held)| {
                    json!([json_of(key.clone(), false), json_of(held.clone(), false)])
                })
                .collect(),
        ),
        Duck::Union(held) => json_of(*held, false),
        other => json!(format!("{other:?}")),
    }
}

command! {
    pub QUERY_RUN = ["lake", "query", "run"], Read,
    "Run SQL (DuckDB) over Data Lake Parquet files bound as tables with --table",
    keywords: ["select", "count", "nulls", "sample", "compare", "duckdb", "parquet", "analyze", "sql"],
    example: "lake query run 'select count(*) from orders where customer_id is null' --table orders=contosolake/raw/sales/orders/date=2026-09-29",
    timeout: 100,
    run: query_run,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::LAKE;
    use crate::lake::testing::{dir, file, listing, parquet};
    use crate::testing::{self, azure};

    #[test]
    fn a_glob_matches_names_and_a_binding_is_an_identifier() {
        assert!(glob(b"date=2026-09-2*", b"date=2026-09-29"));
        assert!(glob(b"part-?.parquet", b"part-1.parquet"));
        assert!(!glob(b"date=2026-09-2*", b"date=2026-10-01"));
        assert_eq!(
            binding("o=contosolake/raw/x").unwrap(),
            ("o".into(), "contosolake/raw/x".into())
        );
        for raw in ["contosolake/raw/x", "1o=x", "o x=y", "o="] {
            assert!(binding(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn a_folder_binds_as_a_view_with_its_partition_column_and_the_sql_runs_over_it() {
        let day = parquet(
            "SELECT * FROM (VALUES (88121, 'c-1'), (88122, 'c-2'), (88123, NULL), (88124, 'c-4')) t(order_id, customer_id)",
        );
        let (outcome, transport) = azure(
            &[LAKE],
            &[
                "lake",
                "query",
                "run",
                "select date, count(*) as orders, count(*) filter (where customer_id is null) as no_customer from o group by date",
                "--table",
                "o=contosolake/raw/sales/orders/date=2026-09-2*",
            ],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                listing(vec![
                    dir("sales/orders/date=2026-09-29"),
                    dir("sales/orders/date=2026-10-01"),
                ]),
                listing(vec![file(
                    "sales/orders/date=2026-09-29/part-0001.parquet",
                    day.len(),
                )]),
                Answer {
                    status: 206,
                    ..Answer::bytes(&day)
                },
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["results"][0]["columns"],
            json!(["date", "orders", "no_customer"])
        );
        assert_eq!(got["results"][0]["rows"], json!([["2026-09-29", 4, 1]]));
        assert_eq!(got["results"][0]["types"][0], "DATE");
        assert_eq!(
            got["read"],
            json!({"files": 1, "bytes": day.len(), "cached": 0})
        );
        let sent = transport.sent();
        assert!(
            sent[2].url.contains(
                "recursive=true&maxResults=5000&directory=sales%2Forders%2Fdate%3D2026-09-29"
            ),
            "{}",
            sent[2].url
        );
        let range = format!("bytes=0-{}", day.len() - 1);
        assert!(
            sent[3].headers.contains(&("x-ms-range".into(), range)),
            "{:?}",
            sent[3].headers
        );
    }

    #[test]
    fn past_max_mb_nothing_downloads_and_the_hint_names_partition_list() {
        let (outcome, transport) = azure(
            &[LAKE],
            &[
                "lake",
                "query",
                "run",
                "select 1",
                "--table",
                "o=contosolake/raw/sales/orders",
                "--max-mb",
                "1",
            ],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                Answer::ok("").with_header("x-ms-resource-type", "directory"),
                listing(vec![file(
                    "sales/orders/date=2026-09-29/part-0001.parquet",
                    3 << 20,
                )]),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("3.0 MiB in 1 files, past --max-mb 1"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("agent-cli lake partition list contosolake/raw/sales/orders"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 3);
    }

    #[test]
    fn values_print_as_sql_query_run_prints_them() {
        let connection = duckdb::Connection::open_in_memory().unwrap();
        let set = collect(
            &connection,
            "select 9007199254740993::BIGINT as big, 1.50::DECIMAL(9,2) as money, TIMESTAMPTZ '2026-09-29 06:02:11+00' as at, \
             TIMESTAMP '2026-09-29 06:02:11' as naive, [1, 2] as list, {'a': 1} as s from range(3)",
            2,
        )
        .unwrap();
        assert_eq!(
            set.types[..3],
            [
                "BIGINT".to_owned(),
                "DECIMAL(9,2)".to_owned(),
                "TIMESTAMPTZ".to_owned()
            ]
        );
        assert_eq!(
            set.rows[0],
            [
                json!("9007199254740993"),
                json!("1.50"),
                json!("2026-09-29T06:02:11Z"),
                json!("2026-09-29T06:02:11"),
                json!([1, 2]),
                json!({"a": 1})
            ]
        );
        assert!(set.truncated);
    }
}
