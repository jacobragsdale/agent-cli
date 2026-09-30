use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::config::Sql;

#[derive(clap::Args)]
pub struct NoArgs {}

/// A configured connection. There is no field a credential could be in.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ConnectionRow {
    name: String,
    /// mssql or oracle.
    kind: &'static str,
    host: String,
    port: u16,
    database: Option<String>,
    /// Oracle's service name.
    service: Option<String>,
    user: String,
    /// Writes are refused on it.
    read_only: bool,
}

fn connection_list(ctx: &Ctx, _: NoArgs) -> Result<Vec<ConnectionRow>> {
    Ok(Sql::load(ctx.config())?
        .connections
        .into_iter()
        .map(|spec| ConnectionRow {
            name: spec.name,
            kind: spec.kind.as_str(),
            host: spec.host,
            port: spec.port,
            database: spec.database,
            service: spec.service,
            user: spec.user,
            read_only: spec.read_only,
        })
        .collect())
}

command! {
    pub CONNECTION_LIST = ["sql", "connection", "list"], Read,
    "List the configured database connections (never their passwords)",
    keywords: ["databases", "servers", "configured", "conn", "names"],
    example: "sql connection list --fields name,kind,host,read_only",
    run: connection_list,
}

#[cfg(test)]
mod tests {

    use crate::testing::{setup, sql};

    #[test]
    fn connection_list_never_shows_a_password_or_where_one_comes_from() {
        let outcome = sql(&["sql", "connection", "list"], setup());
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let listed = outcome.json();
        assert_eq!(listed[0]["name"], "ms");
        assert_eq!(listed[1]["service"], "FREEPDB1");
        assert_eq!(listed[1]["read_only"], true);
        for secret in [
            "s3cret-literal",
            "pass show",
            "CONTOSO_DB_PASSWORD",
            "password",
        ] {
            assert!(
                !outcome.stdout.contains(secret),
                "{secret}: {}",
                outcome.stdout
            );
        }
    }
}
