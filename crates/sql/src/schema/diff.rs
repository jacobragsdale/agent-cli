//! `sql schema diff`: two schemas (one in qa's database and one in prod's,
//! or two in one): the objects on one side only, and each table, module and
//! sequence as `sql object diff` compares it, read for the whole schema in
//! one query per part, so a schema of 500 tables costs what one table does.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, DiffArgs, Side, command, diff, scoped};
use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::catalog::{
    Filter, ObjectKind, ObjectRow, TableRows, by_table, constraints_sql, extras_sql, grants_sql,
    indexes_sql, objects_sql, parse_columns, parse_objects, parse_source, rows, rows_sql,
    schema_columns_sql, schema_sources_sql, sequence_row, sequence_sql, table_row, triggers_sql,
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
    let (left, right) = (side(left)?, side(right)?);
    diff(ctx, &args.diff, &RULES, left, right)
}

/// Each part of every table in a schema, by table.
struct Parts {
    columns: BTreeMap<String, Vec<Vec<Value>>>,
    extras: BTreeMap<String, Vec<Vec<Value>>>,
    constraints: BTreeMap<String, Vec<Vec<Value>>>,
    indexes: BTreeMap<String, Vec<Vec<Value>>>,
    triggers: BTreeMap<String, Vec<Vec<Value>>>,
    grants: BTreeMap<String, Vec<Vec<Value>>>,
    rows: BTreeMap<String, Vec<Vec<Value>>>,
    sequences: BTreeMap<String, Vec<Vec<Value>>>,
}

/// A schema as its diff compares it: each object by name, as `object diff`
/// compares it.
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
    // The columns read puts the table first; every other read puts it last.
    let mut columns: BTreeMap<String, Vec<Vec<Value>>> = BTreeMap::new();
    for row in rows(session, &schema_columns_sql(backend, schema), ctx)? {
        columns
            .entry(text(&row, 0))
            .or_default()
            .push(row[1..].to_vec());
    }
    let mut read = |sql: String| -> Result<BTreeMap<String, Vec<Vec<Value>>>> {
        Ok(by_table(rows(session, &sql, ctx)?))
    };
    let parts = Parts {
        columns,
        extras: read(extras_sql(backend, schema, None))?,
        constraints: read(constraints_sql(backend, schema, None))?,
        indexes: read(indexes_sql(backend, schema, None))?,
        triggers: read(triggers_sql(backend, schema, None))?,
        grants: read(grants_sql(backend, schema, None))?,
        rows: read(rows_sql(backend, schema, None))?,
        sequences: read(sequence_sql(backend, schema, None))?,
    };
    let mut sources = Vec::new();
    for query in schema_sources_sql(backend, schema) {
        sources.extend(rows(session, &query, ctx)?);
    }
    Ok(assemble(backend, &objects, &parts, &sources))
}

/// The schema's row from its objects, its tables' parts and its sources
/// (`name` first).
fn assemble(backend: Kind, objects: &[ObjectRow], parts: &Parts, sources: &[Vec<Value>]) -> Value {
    let mut by_name: BTreeMap<String, Vec<Vec<Value>>> = BTreeMap::new();
    for row in sources {
        by_name
            .entry(text(row, 0))
            .or_default()
            .push(row[1..].to_vec());
    }
    let none: Vec<Vec<Value>> = Vec::new();
    let mut row = Map::new();
    for object in objects {
        let (schema, name, kind) = (&object.schema, &object.name, object.kind);
        let part = |map: &'_ BTreeMap<String, Vec<Vec<Value>>>| -> Vec<Vec<Value>> {
            map.get(name).cloned().unwrap_or_default()
        };
        let held = match kind {
            ObjectKind::Table => {
                let (extras, constraints, indexes) = (
                    part(&parts.extras),
                    part(&parts.constraints),
                    part(&parts.indexes),
                );
                let (triggers, grants, counted) = (
                    part(&parts.triggers),
                    part(&parts.grants),
                    part(&parts.rows),
                );
                table_row(
                    backend,
                    &TableRows {
                        columns: parse_columns(backend, parts.columns.get(name).unwrap_or(&none)),
                        extras: &extras,
                        constraints: &constraints,
                        indexes: &indexes,
                        triggers: &triggers,
                        grants: &grants,
                        rows: &counted,
                    },
                )
            }
            ObjectKind::Sequence => parts
                .sequences
                .get(name)
                .and_then(|rows| rows.first())
                .map_or_else(|| json!({"kind": kind}), |row| sequence_row(backend, row)),
            _ => match by_name.get(name) {
                Some(source) => {
                    json!({"kind": kind, "text": parse_source(backend, schema, name, source)})
                }
                None => json!({"kind": kind}),
            },
        };
        row.insert(name.clone(), held);
    }
    Value::Object(row)
}

command! {
    pub SCHEMA_DIFF = ["sql", "schema", "diff"], Read,
    "Compare two schemas: objects on one side only, columns, keys, indexes, source",
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
    fn a_schema_row_holds_each_table_as_object_diff_reads_it_and_each_modules_source() {
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
        let one = |table: &str, rows: Vec<Vec<Value>>| BTreeMap::from([(table.to_owned(), rows)]);
        let parts = Parts {
            columns: one(
                "orders",
                vec![
                    cells(&[
                        json!("id"),
                        json!("int"),
                        json!(4),
                        json!(10),
                        json!(0),
                        json!("N"),
                        json!("Y"),
                    ]),
                    cells(&[
                        json!("note"),
                        json!("nvarchar"),
                        json!(200),
                        json!(0),
                        json!(0),
                        json!("Y"),
                        json!("N"),
                    ]),
                ],
            ),
            extras: BTreeMap::new(),
            constraints: one(
                "orders",
                vec![cells(&[
                    json!("PK__orders__3213E83F"),
                    json!("P"),
                    json!("id"),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    json!(1),
                    json!("orders"),
                ])],
            ),
            indexes: one(
                "orders",
                vec![cells(&[
                    json!("ix_orders_note"),
                    json!("index"),
                    json!("note"),
                    json!("N"),
                    Value::Null,
                    json!("orders"),
                ])],
            ),
            triggers: BTreeMap::new(),
            grants: BTreeMap::new(),
            rows: one("orders", vec![cells(&[json!(12), json!("orders")])]),
            sequences: one(
                "order_seq",
                vec![cells(&[
                    json!("bigint"),
                    json!("1"),
                    json!("1"),
                    json!("1"),
                    json!("9223372036854775807"),
                    json!(false),
                    json!("57"),
                    json!("50"),
                    json!("order_seq"),
                ])],
            ),
        };
        let sources = [cells(&[
            json!("sp_load"),
            json!("\r\nCREATE PROCEDURE dbo.sp_load AS SELECT 1\r\n"),
        ])];
        let row = assemble(Kind::Mssql, &objects, &parts, &sources);
        assert_eq!(
            row,
            json!({
                "orders": {"kind": "table", "columns": [
                    {"name": "id", "type": "int", "nullable": false},
                    {"name": "note", "type": "nvarchar(100)", "nullable": true},
                ], "primary_key": "id", "unique": [], "foreign_keys": {}, "checks": {},
                "indexes": {"ix_orders_note": {"unique": false, "columns": ["note"], "include": []}},
                "triggers": {}, "grants": {}, "rows": 12},
                "sp_load": {"kind": "procedure", "text": "CREATE PROCEDURE dbo.sp_load AS SELECT 1"},
                "order_seq": {"kind": "sequence", "type": "bigint", "increment": "1", "min": "1",
                    "max": "9223372036854775807", "cycle": false, "cache": "50"},
            })
        );
    }
}
