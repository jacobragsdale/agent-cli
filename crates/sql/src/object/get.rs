//! `sql object get`.

use agent_cli_core::{Ctx, Failure, command, scoped};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::catalog::{
    ColumnInfo, ObjectKind, ObjectRow, columns_sql, object_name, parse_columns, parse_source, rows,
    sequence_ddl, sequence_sql, source_sql, table_ddl,
};
use crate::config::Sql;
use crate::db::{OnConnection, Session};

#[derive(clap::Args)]
pub struct ObjectGetArgs {
    /// Connection name from `sql connection list`; defaults to the only one
    #[arg(long)]
    conn: Option<String>,
    /// [CONN:]schema.name, as a diff prints its sides; either part may be
    /// quoted as [x] or "x"
    object: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Object {
    kind: ObjectKind,
    schema: String,
    name: String,
    /// Source of a view, procedure, function or package (spec, then body).
    text: Option<String>,
    /// A table's columns, in order.
    columns: Vec<ColumnInfo>,
    /// A CREATE sketch of a table (columns, NOT NULL, primary key) or a
    /// sequence.
    ddl: Option<String>,
}

fn object_get(ctx: &Ctx, args: ObjectGetArgs) -> Result<Object> {
    let (scope, object) = scoped(&args.object);
    if let (Some(scope), Some(conn)) = (scope, args.conn.as_deref())
        && scope != conn
    {
        return Err(Failure::usage(format!(
            "{} names connection {scope} but --conn is {conn}",
            args.object
        ))
        .into());
    }
    let (schema, name) = object_name(object).ok_or_else(|| {
        let conn = args
            .conn
            .as_deref()
            .map_or_else(String::new, |conn| format!("--conn {conn} "));
        Failure::usage(format!("expected SCHEMA.NAME, got {:?}", args.object)).hint(format!(
            "agent-cli sql object list {conn}{} --fields id,kind",
            args.object
        ))
    })?;
    let sql = Sql::load(ctx.config())?;
    let spec = sql.connection(ctx, scope.or(args.conn.as_deref()))?;
    let plan = json!({"conn": spec.name, "read": "object", "object": object});
    let op = OnConnection {
        spec,
        client_dir: sql.client_dir.as_deref(),
        deadline: ctx.deadline(),
        plan,
        writes: false,
        work: |session: &mut Session| {
            let found = super::find(session, &spec.name, &schema, &name, ctx)?;
            describe(session, &found, ctx)
        },
    };
    ctx.read(op)
}

/// What there is to say about an object found by [`object_get`].
fn describe(session: &mut Session, found: &ObjectRow, ctx: &Ctx) -> Result<Object> {
    let backend = session.kind;
    let (schema, name) = (found.schema.as_str(), found.name.as_str());
    let mut object = Object {
        kind: found.kind,
        schema: found.schema.clone(),
        name: found.name.clone(),
        text: None,
        columns: Vec::new(),
        ddl: None,
    };
    match found.kind {
        ObjectKind::Table => {
            let columns = parse_columns(
                backend,
                &rows(session, &columns_sql(backend, schema, name), ctx)?,
            );
            object.ddl = Some(table_ddl(backend, schema, name, &columns));
            object.columns = columns;
        }
        ObjectKind::Sequence => {
            let row = rows(session, &sequence_sql(backend, schema, Some(name)), ctx)?;
            object.ddl = row
                .first()
                .map(|row| sequence_ddl(backend, schema, name, row));
        }
        kind => {
            let source = rows(session, &source_sql(backend, schema, name, kind), ctx)?;
            object.text = Some(parse_source(backend, schema, name, &source));
        }
    }
    Ok(object)
}

command! {
    pub OBJECT_GET = ["sql", "object", "get"], Read,
    "Show a view, procedure, function or package's source, or a table's columns",
    keywords: ["definition", "ddl", "code", "describe", "body", "columns"],
    example: "sql object get --conn local-mssql bench.customers --fields kind,columns,ddl",
    run: object_get,
}

#[cfg(test)]
mod tests {
    use crate::testing::{setup, sql};

    #[test]
    fn a_diff_sides_connection_prefix_is_taken_and_one_that_disagrees_with_conn_is_refused() {
        let outcome = sql(
            &["sql", "object", "get", "ms:dbo.orders", "--conn", "ora"],
            setup(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("names connection ms but --conn is ora"),
            "{}",
            outcome.stderr
        );
        let outcome = sql(&["sql", "object", "get", "nope:dbo.orders"], setup());
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("ms, ora, env"),
            "{}",
            outcome.stderr
        );
    }
}
