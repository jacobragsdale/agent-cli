//! The catalog SQL for Postgres, beside the other servers' in `catalog.rs`,
//! `detail.rs` and `sequence.rs`: what each of their functions answers, in
//! the same row layout, read from `pg_catalog`.

use serde_json::Value;

use super::detail::of_table;
use super::{ColumnInfo, Filter, Name, ObjectKind, like, list, quoted};
use crate::db::text;

/// Every object `object list` shows on Postgres, as `ns, name, code`: no
/// partition (part of its table), no identity sequence (part of its column)
/// and nothing an extension made; overloads of a function are one name.
const OBJECTS: &str = "\
                       select c.relnamespace ns, c.relname::text name, \
                       case c.relkind when 'v' then 'view' when 'm' then 'matview' \
                       when 'S' then 'sequence' else 'table' end code \
                       from pg_class c \
                       where c.relkind in ('r', 'p', 'v', 'm', 'S') and not c.relispartition \
                       and not exists (select 1 from pg_depend d where d.classid = 'pg_class'::regclass \
                       and d.objid = c.oid and d.deptype in ('e', 'i')) \
                       union all \
                       select p.pronamespace, p.proname::text, \
                       case p.prokind when 'p' then 'procedure' else 'function' end \
                       from pg_proc p \
                       where p.prokind in ('f', 'p') \
                       and not exists (select 1 from pg_depend d where d.classid = 'pg_proc'::regclass \
                       and d.objid = p.oid and d.deptype = 'e') \
                       group by 1, 2, 3";

/// The `pg_namespace n` predicate for "a schema somebody here made".
pub(crate) const PG_SCHEMAS: &str = "n.nspname not in ('pg_catalog', 'information_schema') \
                                     and n.nspname not like 'pg\\_toast%' and n.nspname not like 'pg\\_temp\\_%'";

/// A Postgres schema as the catalog spells it: as given when one of that
/// spelling exists (a quoted `"MixedCase"` is stored mixed), else folded to
/// lower case the way an unquoted name is.
pub(super) fn namespace(schema: &str) -> String {
    format!(
        "coalesce((select max(nspname) from pg_namespace where nspname = {}), {})",
        quoted(schema),
        quoted(&schema.to_lowercase()),
    )
}

/// A Postgres column's nullability and whether the primary key holds it, as
/// `Y`/`N` the way the other servers' reads say it.
const COLUMN: &str = "case when a.attnotnull then 'N' else 'Y' end, \
                      case when exists (select 1 from pg_index i where i.indrelid = a.attrelid \
                      and i.indisprimary and a.attnum = any(i.indkey)) then 'Y' else 'N' end";

/// What a Postgres view is, as its `CREATE` says it.
const VIEW_KIND: &str = "case c.relkind when 'm' then 'MATERIALIZED VIEW' else 'VIEW' end";

/// What [`objects_sql`] calls each kind. A materialized view is a view
/// whose source says so; a partition is part of its table.
pub(super) fn codes(kind: ObjectKind) -> &'static [&'static str] {
    match kind {
        ObjectKind::Table => &["table"],
        ObjectKind::View => &["view", "matview"],
        ObjectKind::Procedure => &["procedure"],
        ObjectKind::Function => &["function"],
        ObjectKind::Sequence => &["sequence"],
        ObjectKind::Package => &[],
    }
}

pub(super) fn objects_sql(kinds: &[ObjectKind], filter: &Filter) -> String {
    let codes: Vec<&str> = kinds
        .iter()
        .flat_map(|kind| codes(*kind))
        .copied()
        .collect();
    let mut clauses = vec![format!("o.code in ({})", list(&codes))];
    clauses.push(filter.schema.map_or_else(
        || PG_SCHEMAS.to_owned(),
        |schema| format!("n.nspname = {}", namespace(schema)),
    ));
    let mut order = "n.nspname, o.code, o.name".to_owned();
    match filter.name {
        Some(Name::Contains(pattern)) => clauses.push(format!(
            "o.name ilike {} escape '\\'",
            quoted(&like(pattern, false))
        )),
        // An exact spelling wins over the folded one, as on Oracle.
        Some(Name::Exact(name)) => {
            clauses.push(format!(
                "o.name in ({}, {})",
                quoted(name),
                quoted(&name.to_lowercase())
            ));
            order = format!(
                "case when o.name = {} then 0 else 1 end, {order}",
                quoted(name)
            );
        }
        None => {}
    }
    format!(
        "select n.nspname, o.name, o.code, null, count(*) over () \
         from ({OBJECTS}) o join pg_namespace n on n.oid = o.ns \
         where {} order by {order}{}",
        clauses.join(" and "),
        filter
            .limit
            .map_or_else(String::new, |limit| format!(" limit {limit}")),
    )
}

pub(super) fn columns_sql(schema: &str, table: &str) -> String {
    format!(
        "select a.attname, format_type(a.atttypid, a.atttypmod), {COLUMN} \
         from pg_attribute a join pg_class c on c.oid = a.attrelid \
         join pg_namespace n on n.oid = c.relnamespace \
         where n.nspname = {} and c.relname = {} and a.attnum > 0 and not a.attisdropped \
         order by a.attnum",
        quoted(schema),
        quoted(table),
    )
}

/// A row of [`columns_sql`]: `format_type` already spells a type the way
/// psql does (`numeric(12,2)`).
pub(super) fn column(row: &[Value]) -> ColumnInfo {
    ColumnInfo {
        name: text(row, 0),
        type_text: text(row, 1),
        nullable: text(row, 2) == "Y",
        pk: text(row, 3) == "Y",
    }
}

/// A view's query, or each overload of a routine as `pg_get_functiondef`
/// writes it.
pub(super) fn source_sql(schema: &str, name: &str, kind: ObjectKind) -> String {
    if kind == ObjectKind::View {
        format!(
            "select {VIEW_KIND}, pg_get_viewdef(c.oid, true) from pg_class c \
             join pg_namespace n on n.oid = c.relnamespace \
             where n.nspname = {} and c.relname = {} and c.relkind in ('v', 'm')",
            quoted(schema),
            quoted(name),
        )
    } else {
        format!(
            "select 'ROUTINE', pg_get_functiondef(p.oid) from pg_proc p \
             join pg_namespace n on n.oid = p.pronamespace \
             where n.nspname = {} and p.proname = {} and p.prokind = {} \
             order by pg_get_function_identity_arguments(p.oid)",
            quoted(schema),
            quoted(name),
            quoted(if kind == ObjectKind::Procedure {
                "p"
            } else {
                "f"
            }),
        )
    }
}

/// Each source row as the `CREATE` that makes it: a routine's definition
/// is one already.
pub(super) fn parse_source(qualified: &str, rows: &[Vec<Value>]) -> String {
    rows.iter()
        .map(|row| match text(row, 0).as_str() {
            "ROUTINE" => text(row, 1).trim_end().to_owned(),
            view => format!(
                "CREATE {}{view} {qualified} AS\n{}",
                if view == "VIEW" { "OR REPLACE " } else { "" },
                text(row, 1).trim_end().trim_end_matches(';')
            ),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A generated column's expression is in pg_attrdef, as a default is.
pub(super) fn extras_sql(schema: &str, table: Option<&str>) -> String {
    format!(
        "select a.attname, case when a.attgenerated = '' then pg_get_expr(d.adbin, d.adrelid) end, \
         case a.attidentity when 'a' then 'always' when 'd' then 'by default' end, \
         case when a.attgenerated <> '' then pg_get_expr(d.adbin, d.adrelid) end, c.relname \
         from pg_attribute a join pg_class c on c.oid = a.attrelid \
         join pg_namespace n on n.oid = c.relnamespace \
         left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum \
         where n.nspname = {} and c.relkind in ('r', 'p') and a.attnum > 0 \
         and not a.attisdropped{} \
         order by c.relname, a.attnum",
        namespace(schema),
        of_table("c.relname", table),
    )
}

/// Postgres 18 keeps NOT NULL as a constraint too (contype n);
/// nullability says it.
pub(super) fn constraints_sql(schema: &str, table: Option<&str>) -> String {
    format!(
        "select k.conname, case k.contype when 'p' then 'P' when 'u' then 'U' \
         when 'f' then 'R' else 'C' end, \
         a.attname, rn.nspname || '.' || rc.relname, ra.attname, \
         case when k.contype = 'f' then case k.confdeltype when 'a' then 'NO ACTION' \
         when 'r' then 'RESTRICT' when 'c' then 'CASCADE' when 'n' then 'SET NULL' \
         when 'd' then 'SET DEFAULT' end end, \
         case when k.contype = 'c' then pg_get_constraintdef(k.oid, true) end, \
         col.position, c.relname \
         from pg_constraint k join pg_class c on c.oid = k.conrelid \
         join pg_namespace n on n.oid = c.relnamespace \
         left join lateral unnest(k.conkey) with ordinality as col(attnum, position) on true \
         left join pg_attribute a on a.attrelid = k.conrelid and a.attnum = col.attnum \
         left join pg_class rc on rc.oid = k.confrelid \
         left join pg_namespace rn on rn.oid = rc.relnamespace \
         left join pg_attribute ra \
         on ra.attrelid = k.confrelid and ra.attnum = k.confkey[col.position] \
         where n.nspname = {} and k.contype in ('p', 'u', 'f', 'c'){} \
         order by 9, 1, 8",
        namespace(schema),
        of_table("c.relname", table),
    )
}

/// An expression is column 0 of indkey; pg_get_indexdef writes it.
pub(super) fn indexes_sql(schema: &str, table: Option<&str>) -> String {
    format!(
        "select ic.relname, case when i.indisunique then 'unique' else 'index' end, \
         coalesce(a.attname, '') || case when k.opt & 1 = 1 then ' desc' else '' end, \
         case when k.ord > i.indnkeyatts then 'Y' else 'N' end, \
         case when k.attnum = 0 then pg_get_indexdef(i.indexrelid, k.ord::int, true) end, \
         c.relname \
         from pg_index i join pg_class ic on ic.oid = i.indexrelid \
         join pg_class c on c.oid = i.indrelid join pg_namespace n on n.oid = c.relnamespace \
         cross join lateral unnest(i.indkey::int2[], i.indoption::int2[]) \
         with ordinality as k(attnum, opt, ord) \
         left join pg_attribute a on a.attrelid = i.indrelid and a.attnum = k.attnum \
         where n.nspname = {} and c.relkind in ('r', 'p'){} \
         and not exists (select 1 from pg_constraint x where x.conindid = i.indexrelid \
         and x.contype in ('p', 'u', 'x')) \
         order by c.relname, ic.relname, k.ord",
        namespace(schema),
        of_table("c.relname", table),
    )
}

/// A foreign key's own triggers are internal.
pub(super) fn triggers_sql(schema: &str, table: Option<&str>) -> String {
    format!(
        "select t.tgname, pg_get_triggerdef(t.oid, true), c.relname from pg_trigger t \
         join pg_class c on c.oid = t.tgrelid join pg_namespace n on n.oid = c.relnamespace \
         where n.nspname = {} and not t.tgisinternal{} order by c.relname, t.tgname",
        namespace(schema),
        of_table("c.relname", table),
    )
}

/// The owner's own privileges are implied, as on the other servers.
pub(super) fn grants_sql(schema: &str, table: Option<&str>) -> String {
    format!(
        "select coalesce(r.rolname, 'PUBLIC'), g.privilege_type, \
         case when g.is_grantable then 'GRANT_WITH_GRANT_OPTION' else 'GRANT' end, \
         c.relname \
         from pg_class c join pg_namespace n on n.oid = c.relnamespace \
         cross join lateral aclexplode(c.relacl) g \
         left join pg_roles r on r.oid = g.grantee \
         where n.nspname = {} and c.relkind in ('r', 'p') and g.grantee <> c.relowner{} \
         order by c.relname, 1, 2",
        namespace(schema),
        of_table("c.relname", table),
    )
}

/// reltuples is -1 until the table is first analyzed.
pub(super) fn rows_sql(schema: &str, table: Option<&str>) -> String {
    format!(
        "select case when c.reltuples < 0 then null else c.reltuples::bigint end, c.relname \
         from pg_class c join pg_namespace n on n.oid = c.relnamespace \
         where n.nspname = {} and c.relkind in ('r', 'p'){}",
        namespace(schema),
        of_table("c.relname", table),
    )
}

pub(super) fn schema_columns_sql(schema: &str) -> String {
    format!(
        "select c.relname, a.attname, format_type(a.atttypid, a.atttypmod), {COLUMN} \
         from pg_attribute a join pg_class c on c.oid = a.attrelid \
         join pg_namespace n on n.oid = c.relnamespace \
         where n.nspname = {} and c.relkind in ('r', 'p') and not c.relispartition \
         and a.attnum > 0 and not a.attisdropped \
         order by c.relname, a.attnum",
        namespace(schema),
    )
}

pub(super) fn schema_sources_sql(schema: &str) -> Vec<String> {
    vec![
        format!(
            "select p.proname, 'ROUTINE', pg_get_functiondef(p.oid) from pg_proc p \
             join pg_namespace n on n.oid = p.pronamespace \
             where n.nspname = {} and p.prokind in ('f', 'p') \
             and not exists (select 1 from pg_depend d where d.classid = 'pg_proc'::regclass \
             and d.objid = p.oid and d.deptype = 'e') \
             order by p.proname, pg_get_function_identity_arguments(p.oid)",
            namespace(schema),
        ),
        format!(
            "select c.relname, {VIEW_KIND}, pg_get_viewdef(c.oid, true) from pg_class c \
             join pg_namespace n on n.oid = c.relnamespace \
             where n.nspname = {} and c.relkind in ('v', 'm') order by c.relname",
            namespace(schema),
        ),
    ]
}

/// last_value is NULL before the first nextval, or without the right
/// to read the sequence.
pub(super) fn sequence_sql(schema: &str, name: Option<&str>) -> String {
    format!(
        "select data_type::text, start_value::text, increment_by::text, min_value::text, \
         max_value::text, cycle, last_value::text, cache_size::text, sequencename::text \
         from pg_sequences where schemaname = {}{}",
        namespace(schema),
        of_table("sequencename", name),
    )
}
