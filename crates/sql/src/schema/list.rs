//! `sql schema list`.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::catalog::{OWNERS, catalog_read};
use crate::config::{Kind, Sql};
use crate::db::text;

#[derive(clap::Args)]
pub struct SchemaListArgs {
    /// Connection name from `sql connection list`; defaults to the only one
    #[arg(long)]
    conn: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// Schema names: on SQL Server all but the system ones; on Oracle every
/// account Oracle did not create, and the one logged in as.
fn schema_list(ctx: &Ctx, args: SchemaListArgs) -> Result<Vec<String>> {
    let sql = Sql::load(ctx.config())?;
    let spec = sql.connection(args.conn.as_deref())?;
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
