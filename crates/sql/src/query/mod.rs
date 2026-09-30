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

pub(crate) mod bench;
pub(crate) mod run;

use std::path::Path;
use std::time::Duration;

use agent_cli_core::{Ctx, Effect, Failure};
use anyhow::Result;
use serde_json::json;

use crate::config::Connection;
use crate::db::{OnConnection, ResultSet, Session};
use crate::split::{self, Statement};

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
    use serde_json::json;

    use super::*;
    use crate::testing::{setup, sql};

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

    #[test]
    fn a_write_under_dry_run_prints_each_statement_and_its_class_and_connects_to_nothing() {
        for argv in [
            &[
                "sql",
                "query",
                "run",
                "--conn",
                "ms",
                "select 1\ngo\ndelete from t",
            ][..],
            &[
                "sql",
                "query",
                "bench",
                "--conn",
                "ms",
                "--runs",
                "3",
                "update t set a = 1",
            ][..],
        ] {
            let mut argv = argv.to_vec();
            argv.push("--dry-run");
            let outcome = sql(&argv, setup());
            assert_eq!(outcome.code, 0, "{outcome:?}");
            let printed = outcome.json();
            assert_eq!(printed["dry_run"], true, "{outcome:?}");
            let plan = &printed["would"][0];
            assert_eq!(plan["conn"], "ms");
            let last = plan["statements"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()
                .clone();
            assert_eq!(last["writes"], true, "{plan}");
        }
        let outcome = sql(
            &[
                "sql",
                "query",
                "run",
                "--conn",
                "ms",
                "select 1\ngo\ndelete from t",
                "--dry-run",
            ],
            setup(),
        );
        assert_eq!(
            outcome.json()["would"][0]["statements"],
            json!([
                {"sql": "select 1", "writes": false},
                {"sql": "delete from t", "writes": true}
            ])
        );
    }

    #[test]
    fn sql_comes_as_the_argument_piped_with_a_dash_or_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fix.sql");
        std::fs::write(&file, "select 1\r\ngo\r\ndelete from t\r\n").unwrap();
        let file = file.to_str().unwrap();
        for (argv, stdin) in [
            (
                &["sql", "query", "run", "--conn", "ms", "--sql-file", file][..],
                "",
            ),
            (
                &["sql", "query", "bench", "--conn", "ms", "--sql-file", file][..],
                "",
            ),
            (
                &["sql", "query", "run", "--conn", "ms", "-"][..],
                "select 1\ngo\ndelete from t\n",
            ),
        ] {
            let mut argv = argv.to_vec();
            argv.push("--dry-run");
            let outcome = sql(&argv, setup().with_stdin(stdin));
            assert_eq!(outcome.code, 0, "{outcome:?}");
            assert_eq!(
                outcome.json()["would"][0]["statements"],
                json!([
                    {"sql": "select 1", "writes": false},
                    {"sql": "delete from t", "writes": true}
                ])
            );
        }
        for argv in [
            &["sql", "query", "run", "--conn", "ms"][..],
            &[
                "sql",
                "query",
                "run",
                "--conn",
                "ms",
                "select 1",
                "--sql-file",
                file,
            ][..],
            &["sql", "query", "run", "--conn", "ms", "-"][..],
        ] {
            let outcome = sql(argv, setup());
            assert_eq!(outcome.code, 2, "{outcome:?}");
        }
    }

    #[test]
    fn a_write_needs_yes_and_read_only_mode_refuses_it() {
        let outcome = sql(
            &["sql", "query", "run", "--conn", "ms", "drop table t"],
            setup(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stdout.is_empty());
        assert!(outcome.stderr.contains("--yes"), "{}", outcome.stderr);

        for verb in ["run", "bench"] {
            let outcome = sql(
                &[
                    "sql",
                    "query",
                    verb,
                    "--conn",
                    "ms",
                    "insert into t values (1)",
                    "--yes",
                ],
                setup().read_only(),
            );
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(
                outcome.stderr.contains("AGENT_CLI_READ_ONLY"),
                "{}",
                outcome.stderr
            );
        }
    }
}
