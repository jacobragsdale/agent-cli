//! A sequence's settings: what `object get` prints as DDL and what the
//! diffs compare field by field.

use serde_json::{Value, json};

use super::detail::of_table;
use super::{owner, qualified, quoted};
use crate::config::Kind;
use crate::db::{maybe, text};

/// A sequence's settings as text, so the DDL needs no number rules:
/// SQL Server's `type, start, increment, min, max, cycling, current, cache,
/// name`, Oracle's `min, max, increment, cycle, last number, cache, name`.
pub(crate) fn sequence_sql(backend: Kind, schema: &str, name: Option<&str>) -> String {
    match backend {
        Kind::Mssql => format!(
            "select type_name(q.user_type_id), cast(q.start_value as varchar(40)), \
                    cast(q.increment as varchar(40)), cast(q.minimum_value as varchar(40)), \
                    cast(q.maximum_value as varchar(40)), q.is_cycling, \
                    cast(q.current_value as varchar(40)), \
                    case when q.is_cached = 0 then '0' else cast(q.cache_size as varchar(40)) end, \
                    q.name \
             from sys.sequences q join sys.schemas s on s.schema_id = q.schema_id \
             where s.name = {}{}",
            quoted(schema),
            of_table("q.name", name),
        ),
        Kind::Oracle => format!(
            "select to_char(min_value), to_char(max_value), to_char(increment_by), \
                    cycle_flag, to_char(last_number), to_char(cache_size), sequence_name \
             from all_sequences where sequence_owner = {}{}",
            owner(schema),
            of_table("sequence_name", name),
        ),
    }
}

/// `CREATE SEQUENCE` as it stands now: Oracle's `START WITH` is its
/// `LAST_NUMBER`, the next value a new cache would begin at, the way
/// `DBMS_METADATA` writes it; SQL Server's current value follows as a note.
pub(crate) fn sequence_ddl(backend: Kind, schema: &str, name: &str, row: &[Value]) -> String {
    let qualified = qualified(backend, schema, name);
    match backend {
        Kind::Mssql => format!(
            "CREATE SEQUENCE {qualified} AS {} START WITH {} INCREMENT BY {} MINVALUE {} MAXVALUE {} {}\n-- current value {}",
            text(row, 0),
            text(row, 1),
            text(row, 2),
            text(row, 3),
            text(row, 4),
            if row.get(5) == Some(&Value::Bool(true)) {
                "CYCLE"
            } else {
                "NO CYCLE"
            },
            text(row, 6),
        ),
        Kind::Oracle => format!(
            "CREATE SEQUENCE {qualified} START WITH {} INCREMENT BY {} MINVALUE {} MAXVALUE {} {}",
            text(row, 4),
            text(row, 2),
            text(row, 0),
            text(row, 1),
            if text(row, 3) == "Y" {
                "CYCLE"
            } else {
                "NOCYCLE"
            },
        ),
    }
}

/// A sequence as its diff compares it: its settings field by field, never
/// where it stands, which moves with every insert.
pub(crate) fn sequence_row(backend: Kind, row: &[Value]) -> Value {
    let (data_type, increment, min, max, cycle, cache) = match backend {
        Kind::Mssql => (
            maybe(row, 0),
            text(row, 2),
            text(row, 3),
            text(row, 4),
            row.get(5) == Some(&Value::Bool(true)),
            maybe(row, 7),
        ),
        Kind::Oracle => (
            None,
            text(row, 2),
            text(row, 0),
            text(row, 1),
            text(row, 3) == "Y",
            maybe(row, 5),
        ),
    };
    json!({"kind": "sequence", "type": data_type, "increment": increment, "min": min,
        "max": max, "cycle": cycle, "cache": cache})
}
