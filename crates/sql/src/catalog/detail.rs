//! What `sql object diff` and `sql schema diff` compare beyond `object get`:
//! a table's defaults, identity, keys, indexes, checks, triggers and grants,
//! and a sequence's settings. Each query reads one table, or with no table
//! every table of the schema, so a schema costs as many queries as a table;
//! the table's name is each row's last cell. Each engine answers in the same
//! row layout, so one parser reads them all.
//!
//! Constraint names that the server made up (`PK__orders__3213E83F…`,
//! `SYS_C0012345`) differ between two databases built from one script, so
//! keys are compared by their columns and checks by the columns they test,
//! never by name; indexes, which people name, by name.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::{ColumnInfo, owner, postgres, quoted};
use crate::config::Kind;
use crate::db::{maybe, text};

/// ` and COLUMN = 'table'`, or nothing for every table in the schema.
pub(super) fn of_table(column: &str, table: Option<&str>) -> String {
    table.map_or_else(String::new, |table| {
        format!(" and {column} = {}", quoted(table))
    })
}

/// The rows of a schema-wide read by table, the table being each row's last
/// cell.
pub(crate) fn by_table(rows: Vec<Vec<Value>>) -> BTreeMap<String, Vec<Vec<Value>>> {
    let mut tables: BTreeMap<String, Vec<Vec<Value>>> = BTreeMap::new();
    for row in rows {
        let table = row.last().map(|_| text(&row, row.len() - 1));
        tables
            .entry(table.unwrap_or_default())
            .or_default()
            .push(row);
    }
    tables
}

/// Each column's default, identity and computed expression:
/// `name, default, identity, computed` on SQL Server; Oracle gives
/// `name, data_default, identity_column, virtual_column`, its default
/// doubling as a virtual column's expression.
pub(crate) fn extras_sql(backend: Kind, schema: &str, table: Option<&str>) -> String {
    match backend {
        Kind::Mssql => format!(
            "select c.name, dc.definition, \
                    case when c.is_identity = 1 then concat('(', cast(ic.seed_value as varchar(40)), ',', \
                         cast(ic.increment_value as varchar(40)), ')') end, \
                    cc.definition, o.name \
             from sys.columns c join sys.objects o on o.object_id = c.object_id \
             join sys.schemas s on s.schema_id = o.schema_id \
             left join sys.default_constraints dc \
                    on dc.parent_object_id = c.object_id and dc.parent_column_id = c.column_id \
             left join sys.identity_columns ic \
                    on ic.object_id = c.object_id and ic.column_id = c.column_id \
             left join sys.computed_columns cc \
                    on cc.object_id = c.object_id and cc.column_id = c.column_id \
             where s.name = {} and o.type = 'U'{} order by o.name, c.column_id",
            quoted(schema),
            of_table("o.name", table),
        ),
        // data_default is a LONG, which the driver reads as text.
        Kind::Oracle => format!(
            "select column_name, data_default, identity_column, virtual_column, table_name \
             from all_tab_cols where owner = {} and hidden_column = 'NO'{} \
             order by table_name, column_id",
            owner(schema),
            of_table("table_name", table),
        ),
        Kind::Postgres => postgres::extras_sql(schema, table),
    }
}

/// Keys, foreign keys and checks as
/// `name, type (P, U, R, C), column, referenced table, referenced column,
/// delete rule, condition, position, table`; a check's column is the one it
/// was declared on, if any.
pub(crate) fn constraints_sql(backend: Kind, raw_schema: &str, table: Option<&str>) -> String {
    let (schema, on) = (quoted(raw_schema), of_table("o.name", table));
    match backend {
        Kind::Mssql => format!(
            "select kc.name, case kc.type when 'PK' then 'P' else 'U' end, col.name, \
                    null, null, null, null, cast(ic.key_ordinal as int), o.name \
             from sys.key_constraints kc \
             join sys.index_columns ic \
               on ic.object_id = kc.parent_object_id and ic.index_id = kc.unique_index_id \
             join sys.columns col on col.object_id = ic.object_id and col.column_id = ic.column_id \
             join sys.objects o on o.object_id = kc.parent_object_id \
             join sys.schemas s on s.schema_id = o.schema_id \
             where s.name = {schema}{on} \
             union all \
             select fk.name, 'R', pc.name, rs.name + '.' + ro.name, rc.name, \
                    replace(fk.delete_referential_action_desc, '_', ' '), null, \
                    fkc.constraint_column_id, o.name \
             from sys.foreign_keys fk \
             join sys.foreign_key_columns fkc on fkc.constraint_object_id = fk.object_id \
             join sys.columns pc \
               on pc.object_id = fkc.parent_object_id and pc.column_id = fkc.parent_column_id \
             join sys.objects ro on ro.object_id = fk.referenced_object_id \
             join sys.schemas rs on rs.schema_id = ro.schema_id \
             join sys.columns rc \
               on rc.object_id = fkc.referenced_object_id and rc.column_id = fkc.referenced_column_id \
             join sys.objects o on o.object_id = fk.parent_object_id \
             join sys.schemas s on s.schema_id = o.schema_id \
             where s.name = {schema}{on} \
             union all \
             select k.name, 'C', pc.name, null, null, null, k.definition, 0, o.name \
             from sys.check_constraints k join sys.objects o on o.object_id = k.parent_object_id \
             join sys.schemas s on s.schema_id = o.schema_id \
             left join sys.columns pc \
               on pc.object_id = k.parent_object_id and pc.column_id = k.parent_column_id \
             where s.name = {schema}{on} \
             order by 9, 1, 8"
        ),
        // A NOT NULL is a check Oracle names itself; nullability says it.
        Kind::Oracle => format!(
            "select c.constraint_name, c.constraint_type, cc.column_name, \
                    r.owner || '.' || r.table_name, rc.column_name, c.delete_rule, \
                    c.search_condition_vc, cc.position, c.table_name \
             from all_constraints c \
             left join all_cons_columns cc \
               on cc.owner = c.owner and cc.constraint_name = c.constraint_name \
             left join all_constraints r \
               on r.owner = c.r_owner and r.constraint_name = c.r_constraint_name \
             left join all_cons_columns rc on rc.owner = r.owner \
              and rc.constraint_name = r.constraint_name and rc.position = cc.position \
             where c.owner = {}{} \
               and c.constraint_type in ('P', 'U', 'R', 'C') \
               and not (c.constraint_type = 'C' and c.generated = 'GENERATED NAME' \
                        and c.search_condition_vc like '%IS NOT NULL') \
             order by 9, 1, 8",
            owner(raw_schema),
            of_table("c.table_name", table),
        ),
        Kind::Postgres => postgres::constraints_sql(raw_schema, table),
    }
}

/// Indexes that back no key, as `name, unique, column, included,
/// expression, table`. Oracle makes a hidden column (`SYS_NC00008$`, its
/// number differing between databases) for each expression and descending
/// key, so the expression stands in for it.
pub(crate) fn indexes_sql(backend: Kind, schema: &str, table: Option<&str>) -> String {
    match backend {
        Kind::Mssql => format!(
            "select i.name, case when i.is_unique = 1 then 'unique' else 'index' end, \
                    col.name + case when ic.is_descending_key = 1 then ' desc' else '' end, \
                    case when ic.is_included_column = 1 then 'Y' else 'N' end, null, o.name \
             from sys.indexes i \
             join sys.index_columns ic on ic.object_id = i.object_id and ic.index_id = i.index_id \
             join sys.columns col on col.object_id = ic.object_id and col.column_id = ic.column_id \
             join sys.objects o on o.object_id = i.object_id \
             join sys.schemas s on s.schema_id = o.schema_id \
             where s.name = {} and o.type = 'U'{} and i.type > 0 \
               and i.is_primary_key = 0 and i.is_unique_constraint = 0 \
             order by o.name, i.name, ic.is_included_column, ic.key_ordinal, ic.index_column_id",
            quoted(schema),
            of_table("o.name", table),
        ),
        // column_expression is a LONG, which the driver reads as text.
        Kind::Oracle => format!(
            "select i.index_name, case i.uniqueness when 'UNIQUE' then 'unique' else 'index' end, \
                    ic.column_name || case ic.descend when 'DESC' then ' desc' end, 'N', \
                    e.column_expression, i.table_name \
             from all_indexes i join all_ind_columns ic \
               on ic.index_owner = i.owner and ic.index_name = i.index_name \
             left join all_ind_expressions e on e.index_owner = ic.index_owner \
              and e.index_name = ic.index_name and e.column_position = ic.column_position \
             where i.table_owner = {}{} \
               and not exists (select 1 from all_constraints k \
                               where k.owner = i.table_owner and k.index_name = i.index_name) \
             order by i.table_name, i.index_name, ic.column_position",
            owner(schema),
            of_table("i.table_name", table),
        ),
        Kind::Postgres => postgres::indexes_sql(schema, table),
    }
}

/// A table's triggers as `name, source, table` (Oracle: `name, description,
/// body, table`).
pub(crate) fn triggers_sql(backend: Kind, schema: &str, table: Option<&str>) -> String {
    match backend {
        Kind::Mssql => format!(
            "select t.name, object_definition(t.object_id), o.name from sys.triggers t \
             join sys.objects o on o.object_id = t.parent_id \
             join sys.schemas s on s.schema_id = o.schema_id \
             where s.name = {}{} order by o.name, t.name",
            quoted(schema),
            of_table("o.name", table),
        ),
        // trigger_body is a LONG, which the driver reads as text.
        Kind::Oracle => format!(
            "select trigger_name, description, trigger_body, table_name from all_triggers \
             where table_owner = {} and base_object_type = 'TABLE'{} \
             order by table_name, trigger_name",
            owner(schema),
            of_table("table_name", table),
        ),
        Kind::Postgres => postgres::triggers_sql(schema, table),
    }
}

/// What the rows say a table is, beside its columns.
pub(crate) struct TableRows<'a> {
    pub(crate) columns: Vec<ColumnInfo>,
    pub(crate) extras: &'a [Vec<Value>],
    pub(crate) constraints: &'a [Vec<Value>],
    pub(crate) indexes: &'a [Vec<Value>],
    pub(crate) triggers: &'a [Vec<Value>],
    pub(crate) grants: &'a [Vec<Value>],
    pub(crate) rows: &'a [Vec<Value>],
}

/// Who may do what to a table, as `principal, permission, state, table`.
pub(crate) fn grants_sql(backend: Kind, schema: &str, table: Option<&str>) -> String {
    match backend {
        Kind::Mssql => format!(
            "select pr.name, p.permission_name, p.state_desc, o.name \
             from sys.database_permissions p \
             join sys.database_principals pr on pr.principal_id = p.grantee_principal_id \
             join sys.objects o on o.object_id = p.major_id \
             join sys.schemas s on s.schema_id = o.schema_id \
             where p.class = 1 and p.minor_id = 0 and o.type = 'U' and s.name = {}{} \
             order by o.name, pr.name, p.permission_name",
            quoted(schema),
            of_table("o.name", table),
        ),
        Kind::Oracle => format!(
            "select grantee, privilege, case grantable when 'YES' then 'GRANT WITH GRANT OPTION' \
                    else 'GRANT' end, table_name \
             from all_tab_privs where table_schema = {}{} \
             order by table_name, grantee, privilege",
            owner(schema),
            of_table("table_name", table),
        ),
        Kind::Postgres => postgres::grants_sql(schema, table),
    }
}

/// A table's rows as the catalog counts them (Oracle: as of its last
/// statistics), never a scan, as `rows, table`.
pub(crate) fn rows_sql(backend: Kind, schema: &str, table: Option<&str>) -> String {
    match backend {
        Kind::Mssql => format!(
            "select sum(p.rows), o.name from sys.partitions p \
             join sys.objects o on o.object_id = p.object_id \
             join sys.schemas s on s.schema_id = o.schema_id \
             where p.index_id in (0, 1) and o.type = 'U' and s.name = {}{} group by o.name",
            quoted(schema),
            of_table("o.name", table),
        ),
        Kind::Oracle => format!(
            "select num_rows, table_name from all_tables where owner = {}{}",
            owner(schema),
            of_table("table_name", table),
        ),
        Kind::Postgres => postgres::rows_sql(schema, table),
    }
}

/// One key, foreign key or check, gathered from its rows.
#[derive(Default)]
struct Constraint {
    /// P, U, R or C.
    kind: String,
    columns: Vec<String>,
    table: String,
    referenced: Vec<String>,
    /// A foreign key's delete rule, a check's condition.
    said: String,
}

/// A table as its diff compares it.
pub(crate) fn table_row(backend: Kind, rows: &TableRows) -> Value {
    let extras: BTreeMap<String, &Vec<Value>> =
        rows.extras.iter().map(|row| (text(row, 0), row)).collect();
    let columns: Vec<Value> = rows
        .columns
        .iter()
        .map(|column| {
            let mut held = json!({"name": column.name, "type": column.type_text,
                "nullable": column.nullable});
            if let Some(row) = extras.get(&column.name) {
                let (default, identity, computed) = match backend {
                    Kind::Mssql | Kind::Postgres => (maybe(row, 1), maybe(row, 2), maybe(row, 3)),
                    Kind::Oracle => {
                        let identity = text(row, 2) == "YES";
                        let virtual_column = text(row, 3) == "YES";
                        let expression = maybe(row, 1).map(|text| text.trim().to_owned());
                        (
                            expression.clone().filter(|_| !identity && !virtual_column),
                            identity.then(|| "yes".to_owned()),
                            expression.filter(|_| virtual_column),
                        )
                    }
                };
                for (key, value) in [
                    ("default", default),
                    ("identity", identity),
                    ("computed", computed),
                ] {
                    if let Some(value) = value {
                        held[key] = json!(value);
                    }
                }
            }
            held
        })
        .collect();
    let mut constraints: BTreeMap<String, Constraint> = BTreeMap::new();
    for row in rows.constraints {
        let entry = constraints
            .entry(text(row, 0))
            .or_insert_with(|| Constraint {
                kind: text(row, 1),
                ..Constraint::default()
            });
        if let Some(column) = maybe(row, 2).filter(|column| !entry.columns.contains(column)) {
            entry.columns.push(column);
        }
        if let (Some(table), Some(column)) = (maybe(row, 3), maybe(row, 4)) {
            entry.table = table;
            entry.referenced.push(column);
        }
        // A foreign key's delete rule, or a check's condition.
        if let Some(said) = maybe(row, 5).or_else(|| maybe(row, 6)) {
            entry.said = said;
        }
    }
    let mut primary = None;
    let mut unique = Vec::new();
    let mut tests: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut foreign = Map::new();
    for held in constraints.into_values() {
        let named = held.columns.join(", ");
        match held.kind.as_str() {
            "P" => primary = Some(named),
            "U" => unique.push(named),
            "R" => {
                let target = format!(
                    "{} ({}), on delete {}",
                    held.table,
                    held.referenced.join(", "),
                    held.said
                );
                foreign.insert(named, json!(target));
            }
            _ => {
                // Keyed by what it tests, so a changed condition is one
                // change: SQL Server names a table check's columns only in
                // its condition, as `[qty]`.
                let mut tested = held.columns.clone();
                if tested.is_empty() {
                    tested = bracketed(&held.said);
                }
                let key = if tested.is_empty() {
                    "check".to_owned()
                } else {
                    tested.join(", ")
                };
                tests.entry(key).or_default().push(held.said);
            }
        }
    }
    unique.sort();
    // Two checks on one column are told apart by their order, not by the
    // names the server made up.
    let mut checks = Map::new();
    for (key, mut conditions) in tests {
        conditions.sort();
        for (at, condition) in conditions.into_iter().enumerate() {
            let key = if at == 0 {
                key.clone()
            } else {
                format!("{key} ({})", at + 1)
            };
            checks.insert(key, json!(condition));
        }
    }
    let mut indexes = Map::new();
    for index in rows.indexes {
        let entry = indexes.entry(text(index, 0)).or_insert_with(
            || json!({"unique": text(index, 1) == "unique", "columns": [], "include": []}),
        );
        let list = if text(index, 3) == "Y" {
            "include"
        } else {
            "columns"
        };
        let column = match maybe(index, 4) {
            Some(expression) => {
                let expression = expression.trim();
                let plain = expression
                    .strip_prefix('"')
                    .and_then(|rest| rest.strip_suffix('"'))
                    .filter(|name| !name.contains('"'))
                    .unwrap_or(expression);
                let order = if text(index, 2).ends_with(" desc") {
                    " desc"
                } else {
                    ""
                };
                format!("{plain}{order}")
            }
            None => text(index, 2),
        };
        if let Some(held) = entry[list].as_array_mut() {
            held.push(json!(column));
        }
    }
    let triggers: Map<String, Value> = rows
        .triggers
        .iter()
        .map(|trigger| {
            let source = match backend {
                Kind::Mssql | Kind::Postgres => text(trigger, 1),
                Kind::Oracle => format!(
                    "CREATE OR REPLACE TRIGGER {}{}",
                    text(trigger, 1),
                    text(trigger, 2)
                ),
            };
            (text(trigger, 0), json!(source.trim_matches(['\r', '\n'])))
        })
        .collect();
    let mut row = Map::new();
    row.insert("kind".to_owned(), json!("table"));
    row.insert("columns".to_owned(), Value::Array(columns));
    row.insert("primary_key".to_owned(), json!(primary));
    row.insert("unique".to_owned(), json!(unique));
    row.insert("foreign_keys".to_owned(), Value::Object(foreign));
    row.insert("checks".to_owned(), json!(checks));
    row.insert("indexes".to_owned(), Value::Object(indexes));
    row.insert("triggers".to_owned(), Value::Object(triggers));
    let mut grants: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for grant in rows.grants {
        let state = text(grant, 2);
        // A plain grant is the permission; a deny or one with grant option says so.
        let permission = if state == "GRANT" {
            text(grant, 1)
        } else {
            format!(
                "{} ({})",
                text(grant, 1),
                state.to_lowercase().replace('_', " ")
            )
        };
        grants.entry(text(grant, 0)).or_default().push(permission);
    }
    row.insert("grants".to_owned(), json!(grants));
    let counted = rows.rows.first().and_then(|row| maybe(row, 0));
    row.insert(
        "rows".to_owned(),
        json!(counted.and_then(|n| n.parse::<i64>().ok())),
    );
    Value::Object(row)
}

/// The distinct `[name]`s in a SQL Server condition, in order.
fn bracketed(condition: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for piece in condition.split('[').skip(1) {
        if let Some((name, _)) = piece.split_once(']')
            && !names.iter().any(|held| held == name)
        {
            names.push(name.to_owned());
        }
    }
    names
}

/// Every table's columns in a schema, as `table` then `columns_sql`'s.
pub(crate) fn schema_columns_sql(backend: Kind, schema: &str) -> String {
    match backend {
        Kind::Mssql => format!(
            "select o.name, c.name, t.name, c.max_length, c.precision, c.scale, \
                    case when c.is_nullable = 1 then 'Y' else 'N' end, \
                    case when pk.column_id is null then 'N' else 'Y' end \
             from sys.columns c join sys.objects o on o.object_id = c.object_id \
             join sys.schemas s on s.schema_id = o.schema_id \
             join sys.types t on t.user_type_id = c.user_type_id \
             left join (select ic.object_id, ic.column_id \
                        from sys.index_columns ic join sys.key_constraints kc \
                          on kc.parent_object_id = ic.object_id \
                         and kc.unique_index_id = ic.index_id \
                        where kc.type = 'PK') pk \
                    on pk.object_id = c.object_id and pk.column_id = c.column_id \
             where s.name = {} and o.type = 'U' \
             order by o.name, c.column_id",
            quoted(schema),
        ),
        Kind::Oracle => format!(
            "select c.table_name, c.column_name, c.data_type, c.data_length, c.data_precision, \
                    c.data_scale, c.char_used, c.char_length, c.nullable, \
                    case when pk.column_name is null then 'N' else 'Y' end \
             from all_tab_columns c join all_tables tb \
               on tb.owner = c.owner and tb.table_name = c.table_name \
             left join (select cc.owner, cc.table_name, cc.column_name \
                        from all_constraints k \
                        join all_cons_columns cc \
                          on cc.owner = k.owner and cc.constraint_name = k.constraint_name \
                        where k.constraint_type = 'P') pk \
                    on pk.owner = c.owner and pk.table_name = c.table_name \
                   and pk.column_name = c.column_name \
             where c.owner = {} order by c.table_name, c.column_id",
            owner(schema),
        ),
        Kind::Postgres => postgres::schema_columns_sql(schema),
    }
}

/// Every view's, procedure's, function's and package's source in a schema,
/// as `name, then what parse_source reads`.
pub(crate) fn schema_sources_sql(backend: Kind, schema: &str) -> Vec<String> {
    match backend {
        Kind::Mssql => vec![format!(
            "select o.name, object_definition(o.object_id) from sys.objects o \
             join sys.schemas s on s.schema_id = o.schema_id \
             where s.name = {} and o.type in ('V', 'P', 'FN', 'IF', 'TF') order by o.name",
            quoted(schema),
        )],
        Kind::Oracle => vec![
            format!(
                "select name, type, text from all_source where owner = {} \
                 and type in ('PROCEDURE', 'FUNCTION', 'PACKAGE', 'PACKAGE BODY') \
                 order by name, type, line",
                owner(schema),
            ),
            format!(
                "select view_name, 'VIEW', text from all_views where owner = {} order by view_name",
                owner(schema),
            ),
        ],
        Kind::Postgres => postgres::schema_sources_sql(schema),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn column(name: &str, type_text: &str, nullable: bool, pk: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.to_owned(),
            type_text: type_text.to_owned(),
            nullable,
            pk,
        }
    }

    fn row(cells: &[Value]) -> Vec<Value> {
        cells.to_vec()
    }

    #[test]
    fn a_table_is_compared_by_keys_columns_and_conditions_not_made_up_names() {
        let null = Value::Null;
        let extras = [
            row(&[json!("id"), null.clone(), json!("(1,1)"), null.clone()]),
            row(&[
                json!("status"),
                json!("('new')"),
                null.clone(),
                null.clone(),
            ]),
        ];
        let check = |name: &str, column: Value, condition: &str| {
            row(&[
                json!(name),
                json!("C"),
                column,
                null.clone(),
                null.clone(),
                null.clone(),
                json!(condition),
                json!(0),
            ])
        };
        let constraints = [
            check("CK__orders__qty__1A2B", json!("qty"), "([qty]>(0))"),
            check("CK__orders__qty__9F00", json!("qty"), "([qty]<(1000))"),
            check(
                "CK__orders__7C1D",
                null.clone(),
                "([shipped_at]>=[ordered_at])",
            ),
            row(&[
                json!("FK__orders__custo__3C4D"),
                json!("R"),
                json!("customer_id"),
                json!("dbo.customers"),
                json!("id"),
                json!("NO_ACTION"),
                null.clone(),
                json!(1),
            ]),
            row(&[
                json!("PK__orders__3213E83F"),
                json!("P"),
                json!("id"),
                null.clone(),
                null.clone(),
                null.clone(),
                null.clone(),
                json!(1),
            ]),
            row(&[
                json!("uq_orders_ref"),
                json!("U"),
                json!("ref"),
                null.clone(),
                null.clone(),
                null.clone(),
                null.clone(),
                json!(1),
            ]),
        ];
        let indexes = [
            row(&[
                json!("ix_orders_status"),
                json!("index"),
                json!("status"),
                json!("N"),
            ]),
            row(&[
                json!("ix_orders_status"),
                json!("index"),
                json!("created desc"),
                json!("N"),
            ]),
            row(&[
                json!("ix_orders_status"),
                json!("index"),
                json!("total"),
                json!("Y"),
            ]),
        ];
        let triggers = [row(&[
            json!("tr_orders_audit"),
            json!(
                "\r\nCREATE TRIGGER dbo.tr_orders_audit ON dbo.orders AFTER UPDATE AS SELECT 1\r\n"
            ),
        ])];
        let got = table_row(
            Kind::Mssql,
            &TableRows {
                columns: vec![
                    column("id", "int", false, true),
                    column("status", "varchar(20)", false, false),
                ],
                extras: &extras,
                constraints: &constraints,
                indexes: &indexes,
                triggers: &triggers,
                grants: &[
                    row(&[json!("app_reader"), json!("SELECT"), json!("GRANT")]),
                    row(&[json!("app_writer"), json!("INSERT"), json!("GRANT")]),
                    row(&[json!("app_writer"), json!("DELETE"), json!("DENY")]),
                ],
                rows: &[row(&[json!(1204)])],
            },
        );
        assert_eq!(
            got,
            json!({
                "primary_key": "id",
                "kind": "table",
                "columns": [
                    {"name": "id", "type": "int", "nullable": false, "identity": "(1,1)"},
                    {"name": "status", "type": "varchar(20)", "nullable": false, "default": "('new')"},
                ],
                "unique": ["ref"],
                "foreign_keys": {"customer_id": "dbo.customers (id), on delete NO_ACTION"},
                "checks": {
                    "qty": "([qty]<(1000))",
                    "qty (2)": "([qty]>(0))",
                    "shipped_at, ordered_at": "([shipped_at]>=[ordered_at])",
                },
                "indexes": {"ix_orders_status": {"unique": false, "columns": ["status", "created desc"], "include": ["total"]}},
                "triggers": {"tr_orders_audit": "CREATE TRIGGER dbo.tr_orders_audit ON dbo.orders AFTER UPDATE AS SELECT 1"},
                "grants": {"app_reader": ["SELECT"], "app_writer": ["INSERT", "DELETE (deny)"]},
                "rows": 1204,
            })
        );
    }

    #[test]
    fn oracles_identity_and_virtual_columns_are_not_defaults_nor_its_hidden_index_columns_names() {
        let extras = [
            row(&[
                json!("ID"),
                json!("\"BENCH\".\"ISEQ$$_7342\".nextval"),
                json!("YES"),
                json!("NO"),
            ]),
            row(&[
                json!("TOTAL2"),
                json!("\"TOTAL\"*2 "),
                json!("NO"),
                json!("YES"),
            ]),
            row(&[json!("STATUS"), json!("'NEW' "), json!("NO"), json!("NO")]),
        ];
        let got = table_row(
            Kind::Oracle,
            &TableRows {
                columns: vec![
                    column("ID", "NUMBER(10)", false, true),
                    column("TOTAL2", "NUMBER", true, false),
                    column("STATUS", "VARCHAR2(20)", true, false),
                ],
                extras: &extras,
                constraints: &[],
                indexes: &[row(&[
                    json!("IX_ORDERS_AT"),
                    json!("index"),
                    json!("SYS_NC00008$ desc"),
                    json!("N"),
                    json!("\"ORDERED_AT\""),
                ])],
                triggers: &[],
                grants: &[],
                rows: &[],
            },
        );
        // The hidden column's number differs between databases; the
        // expression does not.
        assert_eq!(
            got["indexes"]["IX_ORDERS_AT"]["columns"],
            json!(["ORDERED_AT desc"])
        );
        assert_eq!(
            got["columns"],
            json!([
                {"name": "ID", "type": "NUMBER(10)", "nullable": false, "identity": "yes"},
                {"name": "TOTAL2", "type": "NUMBER", "nullable": true, "computed": "\"TOTAL\"*2"},
                {"name": "STATUS", "type": "VARCHAR2(20)", "nullable": true, "default": "'NEW'"},
            ])
        );
    }
}
