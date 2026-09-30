//! `sql query run`.

use std::path::PathBuf;
use std::time::Instant;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::config::Sql;
use crate::db::{Fetch, OnConnection, ResultSet, Session};

use super::{door, failed_at, millis, plan, statements, summary};

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

#[cfg(test)]
mod tests {

    use crate::testing::{setup, sql};

    #[test]
    fn a_read_only_connection_refuses_a_write_even_with_yes_or_dry_run() {
        for extra in ["--yes", "--dry-run"] {
            let outcome = sql(
                &[
                    "sql",
                    "query",
                    "run",
                    "--conn",
                    "ora",
                    "select 1 from t for update",
                    extra,
                ],
                setup(),
            );
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(
                outcome.stderr.contains("connection \"ora\" is read_only"),
                "{}",
                outcome.stderr
            );
            assert!(outcome.stderr.contains("hint: "), "{}", outcome.stderr);
        }
    }

    #[test]
    fn a_read_is_never_refused_by_read_only_mode_and_goes_to_the_server() {
        let outcome = sql(
            &[
                "sql",
                "query",
                "run",
                "--conn",
                "ms",
                "select 1",
                "--timeout",
                "5",
            ],
            setup().read_only(),
        );
        assert_eq!(outcome.code, 1, "port 9 does not answer: {outcome:?}");
        assert!(!outcome.stderr.contains("READ_ONLY"), "{}", outcome.stderr);
        assert!(
            outcome
                .stderr
                .starts_with("error: cannot connect to 127.0.0.1:9"),
            "{}",
            outcome.stderr
        );
        assert!(!outcome.stderr.contains("s3cret-literal"));
    }
}
