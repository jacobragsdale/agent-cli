//! `sql object list`, `sql object get` and `sql schema list`: what a database
//! says it holds, asked in ordinary SQL down the same path as a query.
//!
//! Identifiers are folded the way each server folds them: Oracle stores an
//! unquoted name upper case, so `bench.order_pkg` is looked up as
//! `BENCH.ORDER_PKG` unless a mixed-case one of that spelling exists; SQL
//! Server keeps the case it was given, so names compare case-insensitively
//! and an exact match wins. Names reach the SQL as quoted literals
//! ([`quoted`]), never as code.

use std::borrow::Cow;

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::config::{Kind, Sql};
use crate::db::{Fetch, OnConnection, Session, maybe, text, whole};

/// What kind of thing an object is. `package` is Oracle's alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ObjectKind {
    Table,
    View,
    Procedure,
    Function,
    Package,
    Sequence,
}

impl ObjectKind {
    const ALL: [Self; 6] = [
        Self::Table,
        Self::View,
        Self::Procedure,
        Self::Function,
        Self::Package,
        Self::Sequence,
    ];

    /// The `sys.objects.type` codes: a SQL Server function is three of them.
    fn mssql_types(self) -> &'static [&'static str] {
        match self {
            Self::Table => &["U"],
            Self::View => &["V"],
            Self::Procedure => &["P"],
            Self::Function => &["FN", "IF", "TF"],
            Self::Sequence => &["SO"],
            Self::Package => &[],
        }
    }

    /// `ALL_OBJECTS.OBJECT_TYPE`. A package body is half of the package that
    /// is already listed, so it is not one.
    fn oracle_type(self) -> &'static str {
        match self {
            Self::Table => "TABLE",
            Self::View => "VIEW",
            Self::Procedure => "PROCEDURE",
            Self::Function => "FUNCTION",
            Self::Package => "PACKAGE",
            Self::Sequence => "SEQUENCE",
        }
    }

    fn from_code(backend: Kind, code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| match backend {
            Kind::Mssql => kind.mssql_types().contains(&code),
            Kind::Oracle => kind.oracle_type() == code,
        })
    }
}

// ---------- sql object list ----------

#[derive(clap::Args)]
pub struct ObjectListArgs {
    /// Connection name from `sql connection list`
    #[arg(long)]
    conn: String,
    /// Only names containing this, any case
    pattern: Option<String>,
    /// Only this schema (owner on Oracle)
    #[arg(long)]
    schema: Option<String>,
    /// Only this kind
    #[arg(long, value_enum)]
    kind: Option<ObjectKind>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, PartialEq, Serialize, JsonSchema)]
pub struct ObjectRow {
    schema: String,
    kind: ObjectKind,
    name: String,
    /// When it last changed, RFC 3339.
    modified: Option<String>,
}

fn object_list(ctx: &Ctx, args: ObjectListArgs) -> Result<Vec<ObjectRow>> {
    let sql = Sql::load(ctx.config())?;
    let spec = sql.connection(&args.conn)?;
    let query = objects_sql(
        spec.kind,
        &Filter {
            schema: args.schema.as_deref(),
            kind: args.kind,
            name: args.pattern.as_deref().map(Name::Contains),
            limit: Some(args.limit),
        },
    );
    let rows = ctx.read(catalog_read(ctx, &sql, spec, "objects", query))?;
    let total = rows.first().map_or(0, |row| whole(row, 4));
    if usize::try_from(total).unwrap_or(usize::MAX) > rows.len() {
        ctx.note(format!(
            "[{} of {total}; --limit N, or narrow with PATTERN, --schema or --kind]",
            rows.len()
        ));
    }
    Ok(parse_objects(spec.kind, &rows))
}

command! {
    pub OBJECT_LIST = ["sql", "object", "list"], Read,
    "List tables and views, procedures, functions, packages and sequences",
    keywords: ["catalog", "find", "search", "browse", "tables", "names"],
    example: "sql object list --conn local-mssql customer --kind table --fields schema,name",
    run: object_list,
}

// ---------- sql object get ----------

#[derive(clap::Args)]
pub struct ObjectGetArgs {
    /// Connection name from `sql connection list`
    #[arg(long)]
    conn: String,
    /// schema.name; either part may be quoted as [x] or "x"
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ColumnInfo {
    name: String,
    /// As the vendor's tools spell it: `nvarchar(100)`, `NUMBER(12,2)`.
    #[serde(rename = "type")]
    type_text: String,
    nullable: bool,
    pk: bool,
}

fn object_get(ctx: &Ctx, args: ObjectGetArgs) -> Result<Object> {
    let (schema, name) = object_name(&args.object).ok_or_else(|| {
        Failure::usage(format!("expected SCHEMA.NAME, got {:?}", args.object)).hint(format!(
            "agent-cli sql object list --conn {} {} --fields schema,name",
            args.conn, args.object
        ))
    })?;
    let sql = Sql::load(ctx.config())?;
    let spec = sql.connection(&args.conn)?;
    let backend = spec.kind;
    let lookup = objects_sql(
        backend,
        &Filter {
            schema: Some(&schema),
            kind: None,
            name: Some(Name::Exact(&name)),
            limit: None,
        },
    );
    let plan = json!({"conn": spec.name, "read": "object", "object": args.object});
    let op = OnConnection {
        spec,
        client_dir: sql.client_dir.as_deref(),
        deadline: ctx.deadline(),
        plan,
        writes: false,
        work: |session: &mut Session| {
            let found = parse_objects(backend, &rows(session, &lookup, ctx)?);
            // Exact first: on a case-sensitive collation two objects can
            // differ by nothing else.
            let found = found
                .iter()
                .find(|object| object.name == name)
                .or_else(|| found.first());
            let Some(found) = found else {
                return Err(Failure::not_found(format!(
                    "no table, view, procedure, function, package or sequence {schema}.{name}"
                ))
                .hint(format!(
                    "agent-cli sql object list --conn {} {name} --fields schema,kind,name",
                    spec.name
                ))
                .into());
            };
            describe(session, found, ctx)
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
            let row = rows(session, &sequence_sql(backend, schema, name), ctx)?;
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

// ---------- sql schema list ----------

#[derive(clap::Args)]
pub struct SchemaListArgs {
    /// Connection name from `sql connection list`
    #[arg(long)]
    conn: String,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// Schema names: on SQL Server all but the system ones; on Oracle every
/// account Oracle did not create, and the one logged in as.
fn schema_list(ctx: &Ctx, args: SchemaListArgs) -> Result<Vec<String>> {
    let sql = Sql::load(ctx.config())?;
    let spec = sql.connection(&args.conn)?;
    let query = match spec.kind {
        Kind::Mssql => "select s.name from sys.schemas s \
             where s.name not in ('sys', 'INFORMATION_SCHEMA', 'guest') \
               and s.name not like 'db[_]%' \
             order by s.name"
            .to_owned(),
        Kind::Oracle => format!("select username from all_users where {OWNERS} order by username"),
    };
    let rows = ctx.read(catalog_read(ctx, &sql, spec, "schemas", query))?;
    let mut schemas: Vec<String> = rows.iter().map(|row| text(row, 0)).collect();
    if schemas.len() > args.limit {
        ctx.note(format!("[{} of {}; --limit N]", args.limit, schemas.len()));
        schemas.truncate(args.limit);
    }
    Ok(schemas)
}

command! {
    pub SCHEMA_LIST = ["sql", "schema", "list"], Read,
    "List the schemas (owners) of a database",
    keywords: ["owners", "users", "namespaces"],
    example: "sql schema list --conn local-mssql",
    run: schema_list,
}

// ---------- the catalog SQL ----------

/// One catalog query on a connection of its own, as a read op.
fn catalog_read<'a>(
    ctx: &'a Ctx,
    sql: &'a Sql,
    spec: &'a crate::config::Connection,
    what: &str,
    query: String,
) -> OnConnection<'a, impl FnOnce(&mut Session) -> Result<Vec<Vec<Value>>> + 'a> {
    OnConnection {
        spec,
        client_dir: sql.client_dir.as_deref(),
        deadline: ctx.deadline(),
        plan: json!({"conn": spec.name, "read": what}),
        writes: false,
        work: move |session: &mut Session| rows(session, &query, ctx),
    }
}

/// Every row of a catalog query.
fn rows(session: &mut Session, sql: &str, ctx: &Ctx) -> Result<Vec<Vec<Value>>> {
    let ran = session.run(sql, Fetch::ALL, ctx.deadline())?;
    Ok(ran.sets.into_iter().flat_map(|set| set.rows).collect())
}

enum Name<'a> {
    Contains(&'a str),
    Exact(&'a str),
}

struct Filter<'a> {
    schema: Option<&'a str>,
    kind: Option<ObjectKind>,
    name: Option<Name<'a>>,
    limit: Option<usize>,
}

/// Objects as `schema, name, type code, modified, total`, where `total` is
/// how many matched before the limit.
fn objects_sql(backend: Kind, filter: &Filter) -> String {
    let kinds: Vec<ObjectKind> = filter
        .kind
        .map_or_else(|| ObjectKind::ALL.to_vec(), |kind| vec![kind]);
    match backend {
        Kind::Mssql => {
            let types: Vec<&str> = kinds
                .iter()
                .flat_map(|kind| kind.mssql_types())
                .copied()
                .collect();
            let mut clauses = vec![
                "o.is_ms_shipped = 0".to_owned(),
                format!("rtrim(o.type) in ({})", list(&types)),
                "s.name not in ('sys', 'INFORMATION_SCHEMA', 'guest')".to_owned(),
            ];
            if let Some(schema) = filter.schema {
                clauses.push(format!("lower(s.name) = lower({})", quoted(schema)));
            }
            let mut order = "s.name, o.type, o.name".to_owned();
            match filter.name {
                Some(Name::Contains(pattern)) => clauses.push(format!(
                    "lower(o.name) like lower({}) escape '\\'",
                    quoted(&like(pattern, true))
                )),
                Some(Name::Exact(name)) => {
                    clauses.push(format!("lower(o.name) = lower({})", quoted(name)));
                    order = format!(
                        "case when o.name = {} then 0 else 1 end, {order}",
                        quoted(name)
                    );
                }
                None => {}
            }
            format!(
                "select {top}s.name, o.name, rtrim(o.type), \
                        convert(varchar(19), o.modify_date, 126), count(*) over () \
                 from sys.objects o join sys.schemas s on s.schema_id = o.schema_id \
                 where {} order by {order}",
                clauses.join(" and "),
                top = filter
                    .limit
                    .map_or_else(String::new, |limit| format!("top ({limit}) ")),
            )
        }
        Kind::Oracle => {
            let types: Vec<&str> = kinds.iter().map(|kind| kind.oracle_type()).collect();
            let mut clauses = vec![format!("o.object_type in ({})", list(&types))];
            clauses.push(filter.schema.map_or_else(
                || format!("o.owner in (select username from all_users where {OWNERS})"),
                |schema| format!("o.owner = {}", owner(schema)),
            ));
            match (filter.name.as_ref(), filter.schema) {
                (Some(Name::Contains(pattern)), _) => clauses.push(format!(
                    "upper(o.object_name) like upper({}) escape '\\'",
                    quoted(&like(pattern, false))
                )),
                (Some(Name::Exact(name)), Some(schema)) => {
                    clauses.push(format!("o.object_name = {}", object(schema, name)));
                }
                (Some(Name::Exact(name)), None) => {
                    clauses.push(format!("o.object_name = {}", quoted(&name.to_uppercase())));
                }
                (None, _) => {}
            }
            format!(
                "select o.owner, o.object_name, o.object_type, \
                        to_char(o.last_ddl_time, 'YYYY-MM-DD\"T\"HH24:MI:SS'), count(*) over () \
                 from all_objects o where {} \
                 order by o.owner, o.object_type, o.object_name{}",
                clauses.join(" and "),
                filter.limit.map_or_else(String::new, |limit| format!(
                    " fetch first {limit} rows only"
                )),
            )
        }
    }
}

/// `%pattern%` for a LIKE, its own wildcards escaped with `\` so they match
/// themselves (SQL Server's `[` is one too).
fn like(pattern: &str, brackets: bool) -> String {
    let mut escaped = String::from("%");
    for character in pattern.chars() {
        if matches!(character, '\\' | '%' | '_') || (brackets && character == '[') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

fn parse_objects(backend: Kind, rows: &[Vec<Value>]) -> Vec<ObjectRow> {
    rows.iter()
        .filter_map(|row| {
            Some(ObjectRow {
                schema: text(row, 0),
                name: text(row, 1),
                kind: ObjectKind::from_code(backend, &text(row, 2))?,
                modified: maybe(row, 3),
            })
        })
        .collect()
}

fn columns_sql(backend: Kind, schema: &str, table: &str) -> String {
    match backend {
        Kind::Mssql => format!(
            "select c.name, t.name, c.max_length, c.precision, c.scale, \
                    case when c.is_nullable = 1 then 'Y' else 'N' end, \
                    case when pk.column_id is null then 'N' else 'Y' end \
             from sys.columns c \
             join sys.objects o on o.object_id = c.object_id \
             join sys.schemas s on s.schema_id = o.schema_id \
             join sys.types t on t.user_type_id = c.user_type_id \
             left join (select ic.object_id, ic.column_id \
                        from sys.index_columns ic \
                        join sys.key_constraints kc \
                          on kc.parent_object_id = ic.object_id \
                         and kc.unique_index_id = ic.index_id \
                        where kc.type = 'PK') pk \
                    on pk.object_id = c.object_id and pk.column_id = c.column_id \
             where s.name = {} and o.name = {} \
             order by c.column_id",
            quoted(schema),
            quoted(table),
        ),
        Kind::Oracle => format!(
            "select c.column_name, c.data_type, c.data_length, c.data_precision, \
                    c.data_scale, c.char_used, c.char_length, c.nullable, \
                    case when pk.column_name is null then 'N' else 'Y' end \
             from all_tab_columns c \
             left join (select cc.owner, cc.table_name, cc.column_name \
                        from all_constraints k \
                        join all_cons_columns cc \
                          on cc.owner = k.owner and cc.constraint_name = k.constraint_name \
                        where k.constraint_type = 'P') pk \
                    on pk.owner = c.owner and pk.table_name = c.table_name \
                   and pk.column_name = c.column_name \
             where c.owner = {} and c.table_name = {} \
             order by c.column_id",
            quoted(schema),
            quoted(table),
        ),
    }
}

fn parse_columns(backend: Kind, rows: &[Vec<Value>]) -> Vec<ColumnInfo> {
    rows.iter()
        .map(|row| match backend {
            Kind::Mssql => ColumnInfo {
                name: text(row, 0),
                type_text: mssql_type_text(
                    &text(row, 1),
                    whole(row, 2),
                    whole(row, 3),
                    whole(row, 4),
                ),
                nullable: text(row, 5) == "Y",
                pk: text(row, 6) == "Y",
            },
            Kind::Oracle => ColumnInfo {
                name: text(row, 0),
                type_text: oracle_type_text(
                    &text(row, 1),
                    whole(row, 2),
                    maybe(row, 3).and_then(|text| text.parse().ok()),
                    maybe(row, 4).and_then(|text| text.parse().ok()),
                    &text(row, 5),
                    whole(row, 6),
                ),
                nullable: text(row, 7) == "Y",
                pk: text(row, 8) == "Y",
            },
        })
        .collect()
}

/// `OBJECT_DEFINITION` is the whole batch that created the object on SQL
/// Server; Oracle keeps a line per row in `ALL_SOURCE`, and a view's text in
/// `ALL_VIEWS`. The names are as the lookup found them, so no folding.
fn source_sql(backend: Kind, schema: &str, name: &str, kind: ObjectKind) -> String {
    match backend {
        Kind::Mssql => format!(
            "select object_definition(o.object_id) from sys.objects o \
             join sys.schemas s on s.schema_id = o.schema_id \
             where s.name = {} and o.name = {}",
            quoted(schema),
            quoted(name),
        ),
        Kind::Oracle if kind == ObjectKind::View => format!(
            "select 'VIEW', text from all_views where owner = {} and view_name = {}",
            quoted(schema),
            quoted(name),
        ),
        // `PACKAGE` sorts before `PACKAGE BODY`, so the spec comes first.
        Kind::Oracle => format!(
            "select type, text from all_source \
             where owner = {} and name = {} and type in ({}) order by type, line",
            quoted(schema),
            quoted(name),
            list(match kind {
                ObjectKind::Package => &["PACKAGE", "PACKAGE BODY"],
                ObjectKind::Function => &["FUNCTION"],
                _ => &["PROCEDURE"],
            }),
        ),
    }
}

fn parse_source(backend: Kind, schema: &str, name: &str, rows: &[Vec<Value>]) -> String {
    let Some(first) = rows.first() else {
        return String::new();
    };
    if backend == Kind::Mssql {
        // NULL is an object created WITH ENCRYPTION.
        return maybe(first, 0).map_or_else(
            || "-- source not available (encrypted)".to_owned(),
            |source| source.trim_matches(['\r', '\n']).trim_end().to_owned(),
        );
    }
    let qualified = qualified(backend, schema, name);
    if text(first, 0) == "VIEW" {
        return format!("CREATE OR REPLACE VIEW {qualified} AS\n{}", text(first, 1));
    }
    let mut parts: Vec<(String, String)> = Vec::new();
    for row in rows {
        let (part, line) = (text(row, 0), text(row, 1));
        match parts.last_mut() {
            Some((last, body)) if *last == part => body.push_str(&line),
            _ => parts.push((part, line)),
        }
    }
    parts
        .iter()
        .map(|(_, body)| format!("CREATE OR REPLACE {}", body.trim_end()))
        .collect::<Vec<_>>()
        .join("\n/\n\n")
}

/// A `CREATE TABLE` from the columns: names, types, nullability and the
/// primary key.
// ponytail: no defaults, identity, checks, foreign keys or indexes; ask
// sys.default_constraints / ALL_CONSTRAINTS if the sketch stops being enough.
fn table_ddl(backend: Kind, schema: &str, name: &str, columns: &[ColumnInfo]) -> String {
    let mut lines: Vec<String> = columns
        .iter()
        .map(|column| {
            format!(
                "    {} {}{}",
                identifier(backend, &column.name),
                column.type_text,
                if column.nullable { "" } else { " NOT NULL" }
            )
        })
        .collect();
    let key: Vec<Cow<'_, str>> = columns
        .iter()
        .filter(|column| column.pk)
        .map(|column| identifier(backend, &column.name))
        .collect();
    if !key.is_empty() {
        lines.push(format!("    PRIMARY KEY ({})", key.join(", ")));
    }
    format!(
        "CREATE TABLE {} (\n{}\n)",
        qualified(backend, schema, name),
        lines.join(",\n")
    )
}

/// A sequence's settings as text, so the DDL needs no number rules.
fn sequence_sql(backend: Kind, schema: &str, name: &str) -> String {
    match backend {
        Kind::Mssql => format!(
            "select type_name(q.user_type_id), cast(q.start_value as varchar(40)), \
                    cast(q.increment as varchar(40)), cast(q.minimum_value as varchar(40)), \
                    cast(q.maximum_value as varchar(40)), q.is_cycling, \
                    cast(q.current_value as varchar(40)) \
             from sys.sequences q join sys.schemas s on s.schema_id = q.schema_id \
             where s.name = {} and q.name = {}",
            quoted(schema),
            quoted(name),
        ),
        Kind::Oracle => format!(
            "select to_char(min_value), to_char(max_value), to_char(increment_by), \
                    cycle_flag, to_char(last_number) \
             from all_sequences where sequence_owner = {} and sequence_name = {}",
            quoted(schema),
            quoted(name),
        ),
    }
}

/// `CREATE SEQUENCE` as it stands now: Oracle's `START WITH` is its
/// `LAST_NUMBER`, the next value a new cache would begin at, the way
/// `DBMS_METADATA` writes it; SQL Server's current value follows as a note.
fn sequence_ddl(backend: Kind, schema: &str, name: &str, row: &[Value]) -> String {
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

/// The `ALL_USERS` predicate for "a schema somebody here made": Oracle
/// ships forty accounts of its own.
const OWNERS: &str = "(oracle_maintained = 'N' \
     or username = sys_context('userenv', 'current_schema'))";

/// SQL Server `sys.types` gives a name, a length in bytes, a precision and
/// a scale; this is the rule SSMS prints them by. `max_length` is -1 for
/// `(max)` and bytes, so the two-byte types are halved.
fn mssql_type_text(name: &str, max_length: i64, precision: i64, scale: i64) -> String {
    let sized = |length: i64| {
        if length < 0 {
            format!("{name}(max)")
        } else {
            format!("{name}({length})")
        }
    };
    match name {
        "nchar" | "nvarchar" => sized(if max_length < 0 { -1 } else { max_length / 2 }),
        "char" | "varchar" | "binary" | "varbinary" => sized(max_length),
        "decimal" | "numeric" => format!("{name}({precision},{scale})"),
        "datetime2" | "datetimeoffset" | "time" => format!("{name}({scale})"),
        _ => name.to_owned(),
    }
}

/// Oracle already spells the scale into timestamp and interval names. What
/// it does not spell is a character type's length (with `CHAR_USED` saying
/// characters or bytes, as `DBMS_METADATA` prints it) or a `NUMBER`'s
/// precision.
fn oracle_type_text(
    data_type: &str,
    data_length: i64,
    precision: Option<i64>,
    scale: Option<i64>,
    char_used: &str,
    char_length: i64,
) -> String {
    match data_type {
        "NCHAR" | "NVARCHAR2" => format!("{data_type}({char_length})"),
        "CHAR" | "VARCHAR2" | "VARCHAR" => match char_used {
            "C" => format!("{data_type}({char_length} CHAR)"),
            _ => format!("{data_type}({data_length} BYTE)"),
        },
        "NUMBER" | "FLOAT" => match (precision, scale) {
            (None, None) => data_type.to_owned(),
            // An INTEGER column: no precision, a scale pinning it whole.
            (None, Some(scale)) => format!("{data_type}(*,{scale})"),
            (Some(precision), None | Some(0)) => format!("{data_type}({precision})"),
            (Some(precision), Some(scale)) => format!("{data_type}({precision},{scale})"),
        },
        "RAW" => format!("RAW({data_length})"),
        _ => data_type.to_owned(),
    }
}

/// An Oracle schema as the catalog spells it: as given when a user of that
/// spelling exists (a quoted `"MixedCase"` is stored mixed), else folded to
/// upper case the way an unquoted name is.
fn owner(schema: &str) -> String {
    format!(
        "coalesce((select max(username) from all_users where username = {}), {})",
        quoted(schema),
        quoted(&schema.to_uppercase()),
    )
}

/// An Oracle object name, spelled the way [`owner`] spells a schema.
fn object(schema: &str, name: &str) -> String {
    format!(
        "coalesce((select max(object_name) from all_objects \
                   where owner = {} and object_name = {}), {})",
        owner(schema),
        quoted(name),
        quoted(&name.to_uppercase()),
    )
}

/// `schema.name`, each part quoted where it has to be.
fn qualified(backend: Kind, schema: &str, name: &str) -> String {
    format!(
        "{}.{}",
        identifier(backend, schema),
        identifier(backend, name)
    )
}

/// A name as it has to be written: as it is when it reads back the same,
/// quoted when a space, a reserved word or (on Oracle, which folds unquoted
/// names to upper case) a lower-case letter would change what it meant.
fn identifier(backend: Kind, name: &str) -> Cow<'_, str> {
    let (reserved, others, open, close) = match backend {
        Kind::Mssql => (MSSQL_RESERVED, "_@$#", '[', ']'),
        Kind::Oracle => (ORACLE_RESERVED, "_$#", '"', '"'),
    };
    let plain = name.starts_with(|first: char| {
        first.is_alphabetic() || (first == '_' && backend == Kind::Mssql)
    }) && name
        .chars()
        .all(|character| character.is_alphanumeric() || others.contains(character))
        && (backend == Kind::Mssql || name.to_uppercase() == name)
        && !reserved
            .split_whitespace()
            .any(|word| word.eq_ignore_ascii_case(name));
    if plain {
        return Cow::Borrowed(name);
    }
    let escaped = name.replace(close, &format!("{close}{close}"));
    Cow::Owned(format!("{open}{escaped}{close}"))
}

/// `Reserved Keywords (Transact-SQL)`.
const MSSQL_RESERVED: &str = "ADD ALL ALTER AND ANY AS ASC AUTHORIZATION BACKUP BEGIN \
    BETWEEN BREAK BROWSE BULK BY CASCADE CASE CHECK CHECKPOINT CLOSE CLUSTERED COALESCE \
    COLLATE COLUMN COMMIT COMPUTE CONSTRAINT CONTAINS CONTAINSTABLE CONTINUE CONVERT CREATE \
    CROSS CURRENT CURRENT_DATE CURRENT_TIME CURRENT_TIMESTAMP CURRENT_USER CURSOR DATABASE \
    DBCC DEALLOCATE DECLARE DEFAULT DELETE DENY DESC DISK DISTINCT DISTRIBUTED DOUBLE DROP \
    DUMP ELSE END ERRLVL ESCAPE EXCEPT EXEC EXECUTE EXISTS EXIT EXTERNAL FETCH FILE \
    FILLFACTOR FOR FOREIGN FREETEXT FREETEXTTABLE FROM FULL FUNCTION GOTO GRANT GROUP HAVING \
    HOLDLOCK IDENTITY IDENTITY_INSERT IDENTITYCOL IF IN INDEX INNER INSERT INTERSECT INTO IS \
    JOIN KEY KILL LEFT LIKE LINENO LOAD MERGE NATIONAL NOCHECK NONCLUSTERED NOT NULL NULLIF \
    OF OFF OFFSETS ON OPEN OPENDATASOURCE OPENQUERY OPENROWSET OPENXML OPTION OR ORDER OUTER \
    OVER PERCENT PIVOT PLAN PRECISION PRIMARY PRINT PROC PROCEDURE PUBLIC RAISERROR READ \
    READTEXT RECONFIGURE REFERENCES REPLICATION RESTORE RESTRICT RETURN REVERT REVOKE RIGHT \
    ROLLBACK ROWCOUNT ROWGUIDCOL RULE SAVE SCHEMA SECURITYAUDIT SELECT \
    SEMANTICKEYPHRASETABLE SEMANTICSIMILARITYDETAILSTABLE SEMANTICSIMILARITYTABLE \
    SESSION_USER SET SETUSER SHUTDOWN SOME STATISTICS SYSTEM_USER TABLE TABLESAMPLE TEXTSIZE \
    THEN TO TOP TRAN TRANSACTION TRIGGER TRUNCATE TRY_CONVERT TSEQUAL UNION UNIQUE UNPIVOT \
    UPDATE UPDATETEXT USE USER VALUES VARYING VIEW WAITFOR WHEN WHERE WHILE WITH WITHIN \
    WRITETEXT";

/// `Oracle SQL Reserved Words`.
const ORACLE_RESERVED: &str = "ACCESS ADD ALL ALTER AND ANY AS ASC AUDIT BETWEEN BY CHAR \
    CHECK CLUSTER COLUMN COLUMN_VALUE COMMENT COMPRESS CONNECT CREATE CURRENT DATE DECIMAL \
    DEFAULT DELETE DESC DISTINCT DROP ELSE EXCLUSIVE EXISTS FILE FLOAT FOR FROM GRANT GROUP \
    HAVING IDENTIFIED IMMEDIATE IN INCREMENT INDEX INITIAL INSERT INTEGER INTERSECT INTO IS \
    LEVEL LIKE LOCK LONG MAXEXTENTS MINUS MLSLABEL MODE MODIFY NESTED_TABLE_ID NOAUDIT \
    NOCOMPRESS NOT NOWAIT NULL NUMBER OF OFFLINE ON ONLINE OPTION OR ORDER PCTFREE PRIOR \
    PUBLIC RAW RENAME RESOURCE REVOKE ROW ROWID ROWNUM ROWS SELECT SESSION SET SHARE SIZE \
    SMALLINT START SUCCESSFUL SYNONYM SYSDATE TABLE THEN TO TRIGGER UID UNION UNIQUE UPDATE \
    USER VALIDATE VALUES VARCHAR VARCHAR2 VIEW WHENEVER WHERE WITH";

/// A SQL string literal: the only way a quote gets into one is doubled.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// `'a', 'b'` for an `in (…)`; none at all is `NULL`, which matches nothing
/// where `in ()` would not parse (SQL Server has no packages).
fn list(values: &[&str]) -> String {
    if values.is_empty() {
        return "NULL".to_owned();
    }
    values
        .iter()
        .map(|value| quoted(value))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `schema.name`, where either part may be quoted as `[x]` or `"x"` (and so
/// hold a dot); a doubled closing mark is the name's own.
fn object_name(text: &str) -> Option<(String, String)> {
    let mut parts = vec![String::new()];
    let mut closing = None;
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match (closing, character) {
            (Some(end), _) if character == end && characters.peek() == Some(&end) => {
                characters.next();
                parts.last_mut()?.push(end);
            }
            (Some(end), _) if character == end => closing = None,
            (None, '[') => closing = Some(']'),
            (None, '"') => closing = Some('"'),
            (None, '.') => parts.push(String::new()),
            _ => parts.last_mut()?.push(character),
        }
    }
    match <[String; 2]>::try_from(parts) {
        Ok([schema, name]) if !schema.is_empty() && !name.is_empty() && closing.is_none() => {
            Some((schema, name))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_name_may_quote_either_part() {
        let pair = |schema: &str, name: &str| Some((schema.to_owned(), name.to_owned()));
        assert_eq!(object_name("bench.customers"), pair("bench", "customers"));
        assert_eq!(
            object_name("[bench].[customers]"),
            pair("bench", "customers")
        );
        assert_eq!(
            object_name("\"BENCH\".\"Mixed.Case\""),
            pair("BENCH", "Mixed.Case")
        );
        assert_eq!(
            object_name("[a]]b].\"say \"\"hi\"\"\""),
            pair("a]b", "say \"hi\"")
        );
        for wrong in ["customers", "a.b.c", ".x", "x.", "[a.b"] {
            assert_eq!(object_name(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn a_name_is_quoted_only_where_it_would_not_read_back_the_same() {
        let mssql = |schema, name| qualified(Kind::Mssql, schema, name);
        assert_eq!(mssql("bench", "customers"), "bench.customers");
        assert_eq!(mssql("dbo", "order details"), "dbo.[order details]");
        assert_eq!(mssql("dbo", "User"), "dbo.[User]");
        assert_eq!(mssql("My Schema", "a]b"), "[My Schema].[a]]b]");
        let oracle = |schema, name| qualified(Kind::Oracle, schema, name);
        assert_eq!(oracle("BENCH", "ORDER$HIST#1"), "BENCH.ORDER$HIST#1");
        assert_eq!(oracle("BENCH", "MixedCase"), "BENCH.\"MixedCase\"");
        assert_eq!(oracle("BENCH", "ORDER"), "BENCH.\"ORDER\"");
    }

    #[test]
    fn a_table_is_written_as_the_create_that_would_make_it() {
        let column = |name: &str, type_text: &str, nullable, pk| ColumnInfo {
            name: name.to_owned(),
            type_text: type_text.to_owned(),
            nullable,
            pk,
        };
        let columns = [
            column("order_id", "int", false, true),
            column("line_no", "int", false, true),
            column("note", "nvarchar(max)", true, false),
        ];
        assert_eq!(
            table_ddl(Kind::Mssql, "dbo", "order lines", &columns),
            "CREATE TABLE dbo.[order lines] (\n    order_id int NOT NULL,\n    \
             line_no int NOT NULL,\n    note nvarchar(max),\n    \
             PRIMARY KEY (order_id, line_no)\n)"
        );
    }

    #[test]
    fn types_are_spelled_the_way_the_vendors_tools_spell_them() {
        assert_eq!(mssql_type_text("nvarchar", 200, 0, 0), "nvarchar(100)");
        assert_eq!(mssql_type_text("varbinary", -1, 0, 0), "varbinary(max)");
        assert_eq!(mssql_type_text("decimal", 9, 12, 2), "decimal(12,2)");
        assert_eq!(mssql_type_text("datetime2", 8, 27, 7), "datetime2(7)");
        assert_eq!(mssql_type_text("int", 4, 10, 0), "int");
        assert_eq!(
            oracle_type_text("VARCHAR2", 400, None, None, "C", 100),
            "VARCHAR2(100 CHAR)"
        );
        assert_eq!(
            oracle_type_text("VARCHAR2", 200, None, None, "B", 200),
            "VARCHAR2(200 BYTE)"
        );
        assert_eq!(
            oracle_type_text("NUMBER", 22, Some(10), Some(0), "", 0),
            "NUMBER(10)"
        );
        assert_eq!(
            oracle_type_text("NUMBER", 22, None, Some(0), "", 0),
            "NUMBER(*,0)"
        );
        assert_eq!(
            oracle_type_text("NUMBER", 22, Some(12), Some(2), "", 0),
            "NUMBER(12,2)"
        );
    }

    #[test]
    fn a_name_reaches_the_sql_only_as_a_literal() {
        assert_eq!(quoted("o'brien"), "'o''brien'");
        let sql = objects_sql(
            Kind::Mssql,
            &Filter {
                schema: Some("x'; drop table t --"),
                kind: Some(ObjectKind::Package),
                name: Some(Name::Contains("50%_[a]")),
                limit: Some(5),
            },
        );
        assert!(
            sql.contains("lower(s.name) = lower('x''; drop table t --')"),
            "{sql}"
        );
        assert!(
            sql.contains("rtrim(o.type) in (NULL)"),
            "no packages on SQL Server: {sql}"
        );
        assert!(
            sql.contains(r"like lower('%50\%\_\[a]%') escape '\'"),
            "{sql}"
        );
        assert!(sql.starts_with("select top (5) "), "{sql}");
        let oracle = objects_sql(
            Kind::Oracle,
            &Filter {
                schema: None,
                kind: None,
                name: Some(Name::Contains("a_b[")),
                limit: Some(50),
            },
        );
        assert!(oracle.contains(r"upper('%a\_b[%')"), "{oracle}");
        assert!(oracle.ends_with(" fetch first 50 rows only"), "{oracle}");
    }

    #[test]
    fn every_type_code_maps_back_to_a_kind() {
        assert_eq!(
            ObjectKind::from_code(Kind::Mssql, "IF"),
            Some(ObjectKind::Function)
        );
        assert_eq!(
            ObjectKind::from_code(Kind::Mssql, "SO"),
            Some(ObjectKind::Sequence)
        );
        assert_eq!(ObjectKind::from_code(Kind::Mssql, "D"), None);
        assert_eq!(
            ObjectKind::from_code(Kind::Oracle, "PACKAGE"),
            Some(ObjectKind::Package)
        );
        assert_eq!(ObjectKind::from_code(Kind::Oracle, "PACKAGE BODY"), None);
    }
}
