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
    indexes_sql, objects_sql, parse_columns, parse_objects, parse_source, rows, sequence_ddl,
    sequence_sql, source_sql, table_row, triggers_sql,
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
            "no table, view, procedure, function, package or sequence {schema}.{name}"
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
            let extras = rows(session, &extras_sql(backend, schema, name), ctx)?;
            let constraints = rows(session, &constraints_sql(backend, schema, name), ctx)?;
            let indexes = rows(session, &indexes_sql(backend, schema, name), ctx)?;
            let triggers = rows(session, &triggers_sql(backend, schema, name), ctx)?;
            table_row(
                backend,
                &TableRows {
                    columns,
                    extras: &extras,
                    constraints: &constraints,
                    indexes: &indexes,
                    triggers: &triggers,
                },
            )
        }
        ObjectKind::Sequence => {
            let row = rows(session, &sequence_sql(backend, schema, name), ctx)?;
            let ddl = row
                .first()
                .map(|row| sequence_ddl(backend, schema, name, row));
            // Where it stands now moves with every insert: no difference.
            let ddl = ddl.map(|ddl| {
                let first = ddl.lines().next().unwrap_or_default();
                match first.split_once(" START WITH ") {
                    Some((head, tail)) => {
                        let rest = tail.split_once(' ').map_or("", |(_, rest)| rest);
                        format!("{head} {rest}")
                    }
                    None => first.to_owned(),
                }
            });
            json!({"kind": found.kind, "ddl": ddl})
        }
        kind => {
            let source = rows(session, &source_sql(backend, schema, name, kind), ctx)?;
            json!({"kind": kind, "text": parse_source(backend, schema, name, &source)})
        }
    })
}
