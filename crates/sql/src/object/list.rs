//! `sql object list`.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::catalog::{
    Filter, Name, ObjectKind, ObjectRow, catalog_read, objects_sql, parse_objects,
};
use crate::config::Sql;
use crate::db::whole;

#[derive(clap::Args)]
pub struct ObjectListArgs {
    /// Connection name from `sql connection list`; defaults to the only one
    #[arg(long)]
    conn: Option<String>,
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

fn object_list(ctx: &Ctx, args: ObjectListArgs) -> Result<Vec<ObjectRow>> {
    let sql = Sql::load(ctx.config())?;
    let spec = sql.connection(args.conn.as_deref())?;
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
