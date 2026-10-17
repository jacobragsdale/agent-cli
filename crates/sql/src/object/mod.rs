//! `sql object`: tables, views, procedures, functions, packages and
//! sequences, as the catalog (`crate::catalog`) finds them.

pub(crate) mod diff;
pub(crate) mod get;
pub(crate) mod list;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use serde_json::{Value, json};

use crate::catalog::{
    Filter, Name, ObjectKind, ObjectRow, TableRows, columns_sql, constraints_sql, extras_sql,
    grants_sql, indexes_sql, objects_sql, parse_columns, parse_objects, parse_source, rows,
    rows_sql, sequence_row, sequence_sql, source_sql, table_row, triggers_sql,
};
use crate::db::Session;

/// The object `schema.name` names on this connection, an exact match first:
/// on a case-sensitive collation two objects can differ by nothing else.
fn find(
    session: &mut Session,
    conn: &str,
    schema: &str,
    name: &str,
    ctx: &Ctx,
) -> Result<ObjectRow> {
    let backend = session.kind;
    let lookup = objects_sql(
        backend,
        &Filter {
            schema: Some(schema),
            kind: None,
            name: Some(Name::Exact(name)),
            limit: None,
        },
    );
    let mut found = parse_objects(backend, &rows(session, &lookup, ctx)?);
    let at = found
        .iter()
        .position(|object| object.name == name)
        .unwrap_or(0);
    if found.is_empty() {
        return Err(Failure::not_found(format!(
            "{conn} has no table, view, procedure, function, package or sequence {schema}.{name}"
        ))
        .hint(format!(
            "agent-cli sql object list --conn {conn} {name} --fields id,kind"
        ))
        .into());
    }
    Ok(found.swap_remove(at))
}

/// An object as `object diff` compares it: a table's columns with their
/// defaults and identity, its keys, indexes, checks and triggers; a
/// module's source; a sequence's settings.
fn compared(session: &mut Session, found: &ObjectRow, ctx: &Ctx) -> Result<Value> {
    let backend = session.kind;
    let (schema, name) = (found.schema.as_str(), found.name.as_str());
    Ok(match found.kind {
        ObjectKind::Table => {
            let columns = parse_columns(
                backend,
                &rows(session, &columns_sql(backend, schema, name), ctx)?,
            );
            let one = Some(name);
            let extras = rows(session, &extras_sql(backend, schema, one), ctx)?;
            let constraints = rows(session, &constraints_sql(backend, schema, one), ctx)?;
            let indexes = rows(session, &indexes_sql(backend, schema, one), ctx)?;
            let triggers = rows(session, &triggers_sql(backend, schema, one), ctx)?;
            let grants = rows(session, &grants_sql(backend, schema, one), ctx)?;
            let counted = rows(session, &rows_sql(backend, schema, one), ctx)?;
            table_row(
                backend,
                &TableRows {
                    columns,
                    extras: &extras,
                    constraints: &constraints,
                    indexes: &indexes,
                    triggers: &triggers,
                    grants: &grants,
                    rows: &counted,
                },
            )
        }
        ObjectKind::Sequence => {
            let row = rows(session, &sequence_sql(backend, schema, Some(name)), ctx)?;
            row.first().map_or_else(
                || json!({"kind": found.kind}),
                |row| sequence_row(backend, row),
            )
        }
        kind => {
            let source = rows(session, &source_sql(backend, schema, name, kind), ctx)?;
            json!({"kind": kind, "text": parse_source(backend, schema, name, &source)})
        }
    })
}
