//! `sql object diff`: one table, view, procedure, function, package or
//! sequence in two places (qa's database and prod's, or two schemas): a
//! table's columns by name (type, nullability, default, identity), keys by
//! their columns, foreign keys, indexes, checks and triggers; a module's
//! source by lines, whitespace, keyword case and comments aside unless
//! `--exact`. Both sides must be one kind of server.

use agent_cli_core::{Ctx, Diff, DiffArgs, Failure, Rules, Side, command, diff, scoped};
use anyhow::Result;
use serde_json::json;

use crate::catalog::object_name;
use crate::config::{Connection, Sql};
use crate::db::{OnConnection, Session};

#[derive(clap::Args)]
pub struct ObjectDiffArgs {
    /// One side: [CONN:]schema.name (qa:dbo.orders); no prefix is --conn's
    left: String,
    /// The other side, as left
    right: String,
    /// Connection name from `sql connection list`; defaults to the only one
    #[arg(long)]
    conn: Option<String>,
    #[command(flatten)]
    diff: DiffArgs,
}

/// SQL texts: comments and keyword case are no difference; a table's rows
/// differ between any two databases, so they show only with `--all`.
pub(crate) const RULES: Rules = Rules {
    keys: &[("columns", "name"), ("*.columns", "name")],
    noise: &["rows"],
    comments: &["--", "/*"],
    fold_case: true,
    ..Rules::PLAIN
};

fn object_diff(ctx: &Ctx, args: ObjectDiffArgs) -> Result<Diff> {
    let sql = Sql::load(ctx.config())?;
    let named = |raw: &str| -> Result<(&Connection, String, String)> {
        let (scope, rest) = scoped(raw);
        let spec = sql.connection(scope.or(args.conn.as_deref()))?;
        let (schema, name) = object_name(rest).ok_or_else(|| {
            Failure::usage(format!("expected [CONN:]SCHEMA.NAME, got {raw:?}")).hint(format!(
                "agent-cli sql object list --conn {} --fields id,kind",
                spec.name
            ))
        })?;
        Ok((spec, schema, name))
    };
    let (left, right) = (named(&args.left)?, named(&args.right)?);
    same_kind(left.0, right.0)?;
    let side = |(spec, schema, name): (&Connection, String, String)| -> Result<Side> {
        let op = OnConnection {
            spec,
            client_dir: sql.client_dir.as_deref(),
            deadline: ctx.deadline(),
            plan: json!({"conn": spec.name, "read": "object", "object": format!("{schema}.{name}")}),
            writes: false,
            work: |session: &mut Session| {
                let found = super::find(session, &spec.name, &schema, &name, ctx)?;
                let row = super::compared(session, &found, ctx)?;
                Ok(Side::new(format!("{}:{}", spec.name, found.id), row))
            },
        };
        ctx.read(op)
    };
    let (left, right) = (side(left)?, side(right)?);
    diff(ctx, &args.diff, &RULES, left, right)
}

/// Oracle's catalog and SQL Server's say one thing in different words:
/// every type, default and source would differ.
pub(crate) fn same_kind(left: &Connection, right: &Connection) -> Result<()> {
    if left.kind == right.kind {
        return Ok(());
    }
    Err(Failure::usage(format!(
        "{} is {} and {} is {}: an object is compared only with one on its own kind of server",
        left.name,
        left.kind.as_str(),
        right.name,
        right.kind.as_str()
    ))
    .into())
}

command! {
    pub OBJECT_DIFF = ["sql", "object", "diff"], Read,
    "Compare a table or procedure in two databases: columns, keys, indexes, source",
    keywords: ["migration", "staging", "columns", "definition", "ddl", "drift"],
    example: "sql object diff qa:dbo.orders prod:dbo.orders",
    run: object_diff,
}

#[cfg(test)]
mod tests {
    use crate::testing::{setup, sql};

    #[test]
    fn oracle_against_sql_server_is_refused_and_a_bad_side_says_its_shape() {
        let outcome = sql(
            &["sql", "object", "diff", "ms:dbo.orders", "ora:BENCH.ORDERS"],
            setup(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("ms is mssql and ora is oracle"),
            "{}",
            outcome.stderr
        );
        let outcome = sql(
            &["sql", "object", "diff", "ms:orders", "env:dbo.orders"],
            setup(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("expected [CONN:]SCHEMA.NAME"),
            "{}",
            outcome.stderr
        );
        let outcome = sql(
            &["sql", "object", "diff", "nope:dbo.orders", "ms:dbo.orders"],
            setup(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("ms, ora, env"),
            "{}",
            outcome.stderr
        );
    }
}
