//! `sql query run` and `sql query bench`.
//!
//! The SQL is split ([`split`]) and every statement classified before
//! anything connects. A batch holding any write goes through `ctx.write` as
//! destructive (so `--yes`, `--dry-run` and `AGENT_CLI_READ_ONLY` apply),
//! and a `read_only` connection refuses it outright; a pure read goes
//! through `ctx.read`.
//!
//! Every statement commits as it runs, on both servers. When one fails the
//! command stops there: stdout stays empty, as for any failure, and the
//! error names the statement and what the ones before it already did, which
//! stays done.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Effect, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::config::{Connection, Sql};
use crate::db::{self, Fetch, OnConnection, ResultSet, Session};
use crate::split::{self, Statement};

// ---------- sql query run ----------

#[derive(clap::Args)]
pub struct RunArgs {
    /// Connection name from `sql connection list`; defaults to the only one
    #[arg(long)]
    conn: Option<String>,
    /// Keep at most this many rows of each result set
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u64).range(1..))]
    max_rows: u64,
    /// The SQL, or - to read it from stdin. SQL Server splits at GO lines,
    /// Oracle at ; and / lines
    #[arg(allow_hyphen_values = true, required_unless_present = "sql_file")]
    sql: Option<String>,
    /// The SQL from a file
    #[arg(long)]
    sql_file: Option<PathBuf>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct QueryResult {
    /// One per result set, or one per statement that returned none.
    results: Vec<ResultSet>,
    elapsed_ms: u64,
}

fn query_run(ctx: &Ctx, args: RunArgs) -> Result<QueryResult> {
    let started = Instant::now();
    let sql = Sql::load(ctx.config())?;
    let spec = sql.connection(args.conn.as_deref())?;
    let statements = statements(ctx, args.sql.as_deref(), args.sql_file.as_deref(), spec)?;
    let keep = usize::try_from(args.max_rows).unwrap_or(usize::MAX);
    let deadline = ctx.deadline();
    let op = OnConnection {
        spec,
        client_dir: sql.client_dir.as_deref(),
        deadline,
        plan: plan(spec, &statements, None),
        writes: statements.iter().any(|statement| statement.writes),
        work: |session: &mut Session| {
            let mut results = Vec::new();
            let mut done: Vec<String> = Vec::new();
            for (index, statement) in statements.iter().enumerate() {
                // A read is cut at the cap; a write is read to its end, so
                // a SQL Server batch it is part of runs whole.
                let fetch = Fetch {
                    keep,
                    stop: !statement.writes,
                };
                let ran = session
                    .run(&statement.sql, fetch, deadline)
                    .map_err(|error| match statements.len() {
                        1 => error,
                        count => error.context(failed_at(index, count, &done)),
                    })?;
                done.push(summary(&ran.sets));
                results.extend(ran.sets);
            }
            Ok(results)
        },
    };
    let results = door(ctx, spec, op)?;
    if results.iter().any(|set| set.truncated) {
        ctx.note(format!(
            "[a result set stopped at --max-rows {}; raise it for more]",
            args.max_rows
        ));
    }
    Ok(QueryResult {
        results,
        elapsed_ms: millis(started.elapsed()),
    })
}

command! {
    pub QUERY_RUN = ["sql", "query", "run"], Varies,
    "Run SQL on a connection and return every result set as JSON",
    keywords: ["select", "execute", "statement", "script", "rows", "insert", "update", "delete", "how many"],
    example: "sql query run --conn local-mssql 'select top 5 id, name from bench.customers'",
    run: query_run,
}

// ---------- sql query bench ----------

#[derive(clap::Args)]
pub struct BenchArgs {
    /// Connection name from `sql connection list`; defaults to the only one
    #[arg(long)]
    conn: Option<String>,
    /// How many times to run it on one connection
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..))]
    runs: u32,
    /// The SQL, or - to read it from stdin; every row is read each run
    #[arg(allow_hyphen_values = true, required_unless_present = "sql_file")]
    sql: Option<String>,
    /// The SQL from a file
    #[arg(long)]
    sql_file: Option<PathBuf>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Bench {
    /// Runs finished: fewer than `requested` when --timeout ran out first.
    runs: u32,
    requested: u32,
    /// Rows the last run read.
    rows: u64,
    /// connect (one sample), first_row and total, in milliseconds.
    phases: Vec<Phase>,
}

#[derive(Debug, PartialEq, Serialize, JsonSchema)]
pub struct Phase {
    phase: String,
    min_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

fn query_bench(ctx: &Ctx, args: BenchArgs) -> Result<Bench> {
    let sql = Sql::load(ctx.config())?;
    let spec = sql.connection(args.conn.as_deref())?;
    let statements = statements(ctx, args.sql.as_deref(), args.sql_file.as_deref(), spec)?;
    let deadline = ctx.deadline();
    let op = OnConnection {
        spec,
        client_dir: sql.client_dir.as_deref(),
        deadline,
        plan: plan(spec, &statements, Some(args.runs)),
        writes: statements.iter().any(|statement| statement.writes),
        work: |session: &mut Session| {
            let mut first_row = Vec::new();
            let mut total = Vec::new();
            let mut rows = 0;
            'runs: for _ in 0..args.runs {
                let started = Instant::now();
                let (mut first, mut read) = (None, 0);
                for statement in &statements {
                    // Nothing kept, everything read: what is timed is the
                    // server and the wire.
                    let fetch = Fetch {
                        keep: 0,
                        stop: false,
                    };
                    match session.run(&statement.sql, fetch, deadline) {
                        Ok(ran) => {
                            first = first.or(ran.first_row);
                            read += ran.rows;
                        }
                        Err(error) if db::is_timeout(&error) && !total.is_empty() => break 'runs,
                        Err(error) => return Err(error),
                    }
                }
                let elapsed = started.elapsed();
                total.push(elapsed);
                first_row.push(first.map_or(elapsed, |at| at.duration_since(started)));
                rows = read;
            }
            Ok((session.connect, first_row, total, rows))
        },
    };
    let (connect, mut first_row, mut total, rows) = door(ctx, spec, op)?;
    let runs = u32::try_from(total.len()).unwrap_or(u32::MAX);
    if runs < args.runs {
        ctx.note(format!(
            "[stopped after {runs} of {} runs at the --timeout deadline]",
            args.runs
        ));
    }
    Ok(Bench {
        runs,
        requested: args.runs,
        rows,
        phases: vec![
            phase("connect", &mut [connect]),
            phase("first_row", &mut first_row),
            phase("total", &mut total),
        ],
    })
}

command! {
    pub QUERY_BENCH = ["sql", "query", "bench"], Varies,
    "Time a query over several runs: connect, first row and total latency",
    keywords: ["benchmark", "timing", "latency", "performance", "slow", "fast", "profile"],
    example: "sql query bench --conn local-mssql --runs 5 'select count(*) from bench.orders'",
    run: query_bench,
}

/// One row of the phase table, to a tenth of a millisecond, never rounded up.
fn phase(name: &str, samples: &mut [Duration]) -> Phase {
    samples.sort_unstable();
    let at = |p| {
        let micros = percentile(samples, p).as_micros();
        (micros / 100) as f64 / 10.0
    };
    Phase {
        phase: name.to_owned(),
        min_ms: at(0),
        p50_ms: at(50),
        p95_ms: at(95),
        max_ms: at(100),
    }
}

/// Nearest rank: the sample at `ceil(p/100 * n)`, counting from one, so at
/// twenty runs the answer is a number that was measured.
fn percentile(sorted: &[Duration], p: usize) -> Duration {
    let rank = (p * sorted.len()).div_ceil(100).max(1);
    sorted.get(rank - 1).copied().unwrap_or_default()
}

// ---------- shared ----------

/// The statements in `sql` (`-` is stdin) or `file`, classified. Nothing to
/// run is a usage error.
fn statements(
    ctx: &Ctx,
    sql: Option<&str>,
    file: Option<&Path>,
    spec: &Connection,
) -> Result<Vec<Statement>> {
    let text = ctx
        .long_text("sql", sql, file, None)?
        .map(|sql| sql.text)
        .unwrap_or_default();
    let statements = split::statements(&text, spec.kind);
    if statements.is_empty() {
        return Err(
            Failure::usage("no statement given: only whitespace or comments")
                .hint("agent-cli sql query run --conn NAME 'select 1'")
                .into(),
        );
    }
    Ok(statements)
}

/// What `--dry-run` prints: each statement and whether it writes.
fn plan(spec: &Connection, statements: &[Statement], runs: Option<u32>) -> serde_json::Value {
    let statements: Vec<_> = statements
        .iter()
        .map(|statement| json!({"sql": statement.sql, "writes": statement.writes}))
        .collect();
    let mut plan = json!({"conn": spec.name, "statements": statements});
    if let Some(runs) = runs {
        plan["runs"] = json!(runs);
    }
    plan
}

/// `ctx.read` for a pure read; for a write, the connection's own
/// `read_only` first, then `ctx.write` as destructive.
fn door<T, F: FnOnce(&mut Session) -> Result<T>>(
    ctx: &Ctx,
    spec: &Connection,
    op: OnConnection<'_, F>,
) -> Result<T> {
    if !op.writes {
        return ctx.read(op);
    }
    if spec.read_only {
        let writes: Vec<&str> = op.plan["statements"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|statement| statement["writes"] == true)
            .filter_map(|statement| statement["sql"].as_str())
            .collect();
        let first = writes.first().copied().unwrap_or_default();
        let first: String = first.chars().take(60).collect();
        return Err(Failure::usage(format!(
            "connection {:?} is read_only, and {first:?} may write (only a statement starting \
             with select or with, and holding no insert, update, delete, into, exec … counts \
             as a read)",
            spec.name
        ))
        .hint("run only reads here, or use a connection without read_only = true")
        .into());
    }
    ctx.write(Effect::Destructive, op)
}

/// "statement 3 of 3 failed; statements 1-2 already ran and stay committed
/// (1: 3 rows affected, 2: 5 rows returned)".
fn failed_at(index: usize, count: usize, done: &[String]) -> String {
    let mut message = format!("statement {} of {count} failed", index + 1);
    match done {
        [] => {}
        [only] => message.push_str(&format!(
            "; statement 1 already ran and stays committed ({only})"
        )),
        _ => {
            let ran: Vec<String> = done
                .iter()
                .enumerate()
                .map(|(number, what)| format!("{}: {what}", number + 1))
                .collect();
            message.push_str(&format!(
                "; statements 1-{} already ran and stay committed ({})",
                done.len(),
                ran.join(", ")
            ));
        }
    }
    message
}

fn summary(sets: &[ResultSet]) -> String {
    let rows = |count: u64| match count {
        1 => "1 row".to_owned(),
        count => format!("{count} rows"),
    };
    if sets.iter().any(|set| !set.columns.is_empty()) {
        let returned: usize = sets.iter().map(|set| set.rows.len()).sum();
        format!("{} returned", rows(returned as u64))
    } else {
        let affected: u64 = sets.iter().map(|set| set.rows_affected).sum();
        format!("{} affected", rows(affected))
    }
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_percentile_is_the_nearest_rank_sample() {
        let ms = |values: &[u64]| -> Vec<Duration> {
            values
                .iter()
                .map(|value| Duration::from_millis(*value))
                .collect()
        };
        let five = ms(&[10, 20, 30, 40, 50]);
        assert_eq!(percentile(&five, 0), Duration::from_millis(10));
        assert_eq!(percentile(&five, 50), Duration::from_millis(30));
        assert_eq!(percentile(&five, 95), Duration::from_millis(50));
        let twenty = ms(&(1..=20).collect::<Vec<_>>());
        assert_eq!(percentile(&twenty, 95), Duration::from_millis(19));
        assert_eq!(percentile(&[], 50), Duration::ZERO);
    }

    #[test]
    fn a_phase_is_milliseconds_to_a_tenth_never_rounded_up() {
        let mut samples: Vec<Duration> = [4_000, 150, 9_990, 2_000, 3_049]
            .iter()
            .map(|micros| Duration::from_micros(*micros))
            .collect();
        assert_eq!(
            phase("total", &mut samples),
            Phase {
                phase: "total".to_owned(),
                min_ms: 0.1,
                p50_ms: 3.0,
                p95_ms: 9.9,
                max_ms: 9.9,
            }
        );
    }

    #[test]
    fn a_failure_says_which_statement_and_what_ran_before_it() {
        assert_eq!(failed_at(0, 3, &[]), "statement 1 of 3 failed");
        assert_eq!(
            failed_at(1, 2, &["1 row returned".into()]),
            "statement 2 of 2 failed; statement 1 already ran and stays committed (1 row returned)"
        );
        assert_eq!(
            failed_at(2, 3, &["3 rows affected".into(), "5 rows returned".into()]),
            "statement 3 of 3 failed; statements 1-2 already ran and stay committed (1: 3 rows \
             affected, 2: 5 rows returned)"
        );
        let affected = ResultSet {
            rows_affected: 1,
            ..ResultSet::default()
        };
        assert_eq!(summary(&[affected]), "1 row affected");
    }
}
