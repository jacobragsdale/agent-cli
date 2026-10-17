//! `sql schema diff`: two schemas (one in qa's database and one in prod's,
//! or two in one): the objects on one side only, each table's columns, and
//! each view's, procedure's, function's and package's source by lines. Keys,
//! indexes and triggers are one table's question: `sql object diff`.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, DiffArgs, Side, command, diff, scoped};
use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::catalog::{
    ColumnInfo, Filter, ObjectKind, ObjectRow, objects_sql, parse_columns, parse_objects,
    parse_source, rows, schema_columns_sql, schema_sources_sql,
};
use crate::config::{Connection, Kind, Sql};
use crate::db::{OnConnection, Session, text};
use crate::object::diff::{RULES, same_kind};

#[derive(clap::Args)]
pub struct SchemaDiffArgs {
    /// One side: [CONN:]SCHEMA (qa:dbo); no prefix is --conn's
    left: String,
    /// The other side, as left
    right: String,
    /// Connection name from `sql connection list`; defaults to the only one
    #[arg(long)]
    conn: Option<String>,
    #[command(flatten)]
    diff: DiffArgs,
}

fn schema_diff(ctx: &Ctx, args: SchemaDiffArgs) -> Result<Diff> {
    let sql = Sql::load(ctx.config())?;
    let named = |raw: &str| -> Result<(&Connection, String)> {
        let (scope, rest) = scoped(raw);
        Ok((
            sql.connection(scope.or(args.conn.as_deref()))?,
            rest.to_owned(),
        ))
    };
    let (left, right) = (named(&args.left)?, named(&args.right)?);
    same_kind(left.0, right.0)?;
    let side = |(spec, schema): (&Connection, String)| -> Result<Side> {
        let op = OnConnection {
            spec,
            client_dir: sql.client_dir.as_deref(),
            deadline: ctx.deadline(),
            plan: json!({"conn": spec.name, "read": "schema", "schema": schema}),
            writes: false,
            work: |session: &mut Session| schema_row(session, &schema, ctx),
        };
        Ok(Side::new(format!("{}:{schema}", spec.name), ctx.read(op)?))
    };
    let (left_side, right_side) = (side(left.clone())?, side(right.clone())?);
    let (left_id, right_id) = (left_side.id.clone(), right_side.id.clone());
    let answer = diff(ctx, &args.diff, &RULES, left_side, right_side)?;
    // One object apart: its own diff says how, keys and indexes too.
    let mut objects: Vec<&str> = answer
        .changes
        .iter()
        .filter_map(|change| change.path.split('.').next())
        .collect();
    objects.dedup();
    if let [only] = objects.as_slice() {
        ctx.note(format!(
            "[next: agent-cli sql object diff {left_id}.{only} {right_id}.{only}]"
        ));
    }
    Ok(answer)
}

/// A schema as its diff compares it: each object by name, with a table's
/// columns and a module's source.
fn schema_row(session: &mut Session, schema: &str, ctx: &Ctx) -> Result<Value> {
    let backend = session.kind;
    let filter = Filter {
        schema: Some(schema),
        kind: None,
        name: None,
        limit: None,
    };
    let objects = parse_objects(
        backend,
        &rows(session, &objects_sql(backend, &filter), ctx)?,
    );
    let columns = rows(session, &schema_columns_sql(backend, schema), ctx)?;
    let mut sources = Vec::new();
    for query in schema_sources_sql(backend, schema) {
        sources.extend(rows(session, &query, ctx)?);
    }
    Ok(assemble(backend, &objects, &columns, &sources))
}

/// The schema's row from its objects, its tables' columns (`table` first)
/// and its sources (`name` first).
fn assemble(
    backend: Kind,
    objects: &[ObjectRow],
    columns: &[Vec<Value>],
    sources: &[Vec<Value>],
) -> Value {
    let mut by_table: BTreeMap<String, Vec<Vec<Value>>> = BTreeMap::new();
    for row in columns {
        by_table
            .entry(text(row, 0))
            .or_default()
            .push(row[1..].to_vec());
    }
    let mut by_name: BTreeMap<String, Vec<Vec<Value>>> = BTreeMap::new();
    for row in sources {
        by_name
            .entry(text(row, 0))
            .or_default()
            .push(row[1..].to_vec());
    }
    let mut row = Map::new();
    for object in objects {
        let (schema, name, kind) = (&object.schema, &object.name, object.kind);
        let mut held = json!({"kind": kind});
        if kind == ObjectKind::Table {
            let parsed: Vec<ColumnInfo> =
                parse_columns(backend, by_table.get(name).map_or(&[][..], Vec::as_slice));
            held["columns"] = serde_json::to_value(parsed).unwrap_or_default();
        } else if let Some(source) = by_name.get(name) {
            held["text"] = json!(parse_source(backend, schema, name, source));
        }
        row.insert(name.clone(), held);
    }
    Value::Object(row)
}

command! {
    pub SCHEMA_DIFF = ["sql", "schema", "diff"], Read,
    "Compare two schemas: objects on one side only, table columns, procedure source",
    keywords: ["migration", "ran", "everywhere", "staging", "drift", "tables", "procedures"],
    example: "sql schema diff qa:dbo prod:dbo",
    run: schema_diff,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn cells(values: &[Value]) -> Vec<Value> {
        values.to_vec()
    }

    #[test]
    fn a_schema_row_holds_each_tables_columns_and_each_modules_source() {
        let object = |name: &str, kind| ObjectRow {
            id: format!("dbo.{name}"),
            schema: "dbo".to_owned(),
            kind,
            name: name.to_owned(),
            modified: None,
        };
        let objects = [
            object("orders", ObjectKind::Table),
            object("sp_load", ObjectKind::Procedure),
            object("order_seq", ObjectKind::Sequence),
        ];
        let columns = [
            cells(&[
                json!("orders"),
                json!("id"),
                json!("int"),
                json!(4),
                json!(10),
                json!(0),
                json!("N"),
                json!("Y"),
            ]),
            cells(&[
                json!("orders"),
                json!("note"),
                json!("nvarchar"),
                json!(200),
                json!(0),
                json!(0),
                json!("Y"),
                json!("N"),
            ]),
        ];
        let sources = [cells(&[
            json!("sp_load"),
            json!("\r\nCREATE PROCEDURE dbo.sp_load AS SELECT 1\r\n"),
        ])];
        let row = assemble(Kind::Mssql, &objects, &columns, &sources);
        assert_eq!(
            row,
            json!({
                "orders": {"kind": "table", "columns": [
                    {"name": "id", "type": "int", "nullable": false, "pk": true},
                    {"name": "note", "type": "nvarchar(100)", "nullable": true, "pk": false},
                ]},
                "sp_load": {"kind": "procedure", "text": "CREATE PROCEDURE dbo.sp_load AS SELECT 1"},
                "order_seq": {"kind": "sequence"},
            })
        );
    }
}
