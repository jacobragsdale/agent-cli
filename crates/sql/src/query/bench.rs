//! `sql query bench`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::config::{Kind, Sql};
use crate::db::{self, Fetch, OnConnection, Session};

use super::{door, plan, statements};

#[derive(clap::Args)]
pub struct BenchArgs {
    /// Connection name from `sql connection list`; defaults to the only one
    #[arg(long)]
    conn: Option<String>,
    /// How many times to run it on one connection
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..))]
    runs: u32,
    /// Keep at most this many rows of each result set, ending the read there
    /// (default: read every row each run)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_rows: Option<u64>,
    /// The SQL, or - to read it from stdin
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
    /// Rows the last run read (at most --max-rows of each result set).
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
    let cap = args
        .max_rows
        .map(|rows| usize::try_from(rows).unwrap_or(usize::MAX));
    let mut cut = false;
    // A run the deadline cut off after a write: what it wrote stays.
    let mut cut_after_write = false;
    let writes = statements.iter().any(|statement| statement.writes);
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
                let (mut first, mut read, mut wrote) = (None, 0, false);
                for statement in &statements {
                    // Nothing kept, everything read: what is timed is the
                    // server and the wire. Under --max-rows a read keeps
                    // that many and ends there, as `query run` does.
                    let fetch = match cap {
                        None => Fetch {
                            keep: 0,
                            stop: false,
                        },
                        Some(keep) => Fetch {
                            keep,
                            stop: !statement.writes && !wrote,
                        },
                    };
                    match session.run(&statement.sql, fetch, deadline) {
                        Ok(ran) if cap.is_some() => {
                            first = first.or(ran.first_row);
                            cut |= ran.sets.iter().any(|set| set.truncated);
                            read += ran
                                .sets
                                .iter()
                                .map(|set| set.rows.len() as u64)
                                .sum::<u64>();
                        }
                        Ok(ran) => {
                            first = first.or(ran.first_row);
                            read += ran.rows;
                        }
                        Err(error) if db::is_timeout(&error) && !total.is_empty() => {
                            cut_after_write = wrote;
                            break 'runs;
                        }
                        Err(error) if writes && !total.is_empty() => {
                            return Err(error.context(format!(
                                "run {} of {} failed; the {} before it ran and stay committed",
                                total.len() + 1,
                                args.runs,
                                total.len()
                            )));
                        }
                        Err(error) => return Err(error),
                    }
                    wrote |= statement.writes;
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
    if cut_after_write {
        ctx.note(format!(
            "[run {} was cut off after statements that wrote, and what they did stays]",
            runs + 1
        ));
    }
    if cut && matches!(spec.kind, Kind::Mssql) {
        ctx.note(
            "[--max-rows cut the read short, which ends a SQL Server session: each run after the first connected again, inside its total]",
        );
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

#[cfg(test)]
mod tests {

    use super::*;
    use crate::testing::{setup, sql};

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
    fn bench_takes_max_rows_as_query_run_does() {
        let outcome = sql(
            &[
                "sql",
                "query",
                "bench",
                "--conn",
                "ms",
                "--max-rows",
                "10",
                "--runs",
                "2",
                "update t set a = 1",
                "--dry-run",
            ],
            setup(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["would"][0]["runs"], 2);
        let outcome = sql(
            &[
                "sql",
                "query",
                "bench",
                "--conn",
                "ms",
                "--max-rows",
                "0",
                "select 1",
            ],
            setup(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
