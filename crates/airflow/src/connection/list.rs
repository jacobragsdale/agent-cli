//! `airflow connection list`: Connections' type, host and database
//! (`GET connections?connection_id_pattern=…`), and the configured
//! `[[sql.connection]]` on the same host.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::client::{Airflow, At, note_more, query_value, text};

#[derive(clap::Args)]
pub struct ConnectionListArgs {
    /// Only connection ids containing this (% and _ are wildcards)
    pattern: Option<String>,
    #[command(flatten)]
    at: At,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A Connection without its credentials: no field here can hold one.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ConnectionRow {
    /// The conn_id a DAG names.
    id: String,
    /// The conn_type: mssql, postgres, http, wasb ….
    #[serde(rename = "type")]
    kind: Option<String>,
    host: Option<String>,
    port: Option<i64>,
    /// The database, for a database connection.
    schema: Option<String>,
    description: Option<String>,
    /// The [[sql.connection]] on the same host (and database): what
    /// sql query run --conn takes.
    sql_conn: Option<String>,
}

/// The keys of `[sql]` that `sql_conn` reads; the sql crate checks the rest.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Sql {
    connection: Vec<SqlConnection>,
}

#[derive(Deserialize)]
struct SqlConnection {
    name: String,
    kind: Option<String>,
    host: Option<String>,
    port: Option<i64>,
    database: Option<String>,
}

impl SqlConnection {
    /// The port sql connects to: its own, else its server's default.
    fn port(&self) -> Option<i64> {
        self.port.or(match self.kind.as_deref() {
            Some("mssql") => Some(1433),
            Some("oracle") => Some(1521),
            _ => None,
        })
    }
}

/// The sql connection on `host`, preferring one whose database is
/// `database`; one naming another database, or another port (two servers
/// on one host), never matches.
// ponytail: hosts compare as written; a port or instance suffix on one side
// (`host,1433`, `host\inst`) misses, normalize both when that shows up.
fn sql_conn(
    sql: &[SqlConnection],
    host: Option<&str>,
    port: Option<i64>,
    database: Option<&str>,
) -> Option<String> {
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    let host = host?;
    let on_host: Vec<&SqlConnection> = sql
        .iter()
        .filter(|conn| conn.host.as_deref().is_some_and(|their| same(their, host)))
        .filter(|conn| {
            conn.port()
                .zip(port)
                .is_none_or(|(theirs, ours)| theirs == ours)
        })
        .filter(|conn| match (conn.database.as_deref(), database) {
            (Some(theirs), Some(ours)) => same(theirs, ours),
            _ => true,
        })
        .collect();
    on_host
        .iter()
        .find(|conn| conn.database.is_some() && database.is_some())
        .or(on_host.first())
        .map(|conn| conn.name.clone())
}

fn connection_list(ctx: &Ctx, args: ConnectionListArgs) -> Result<Vec<ConnectionRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    let query = args
        .pattern
        .map(|pattern| format!("connection_id_pattern={}", query_value(pattern.trim())))
        .unwrap_or_default();
    let (connections, total) = client.list("connections", &query, "connections", args.limit)?;
    note_more(ctx, connections.len(), total);
    // A broken [sql] section is sql's to report; here it only means no match.
    let sql: Sql = ctx.config().section("sql").unwrap_or_default();
    Ok(connections
        .iter()
        .map(|connection| {
            let host = text(&connection["host"]);
            let schema = text(&connection["schema"]);
            let port = connection["port"].as_i64();
            ConnectionRow {
                id: connection["connection_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                kind: text(&connection["conn_type"]),
                sql_conn: sql_conn(&sql.connection, host.as_deref(), port, schema.as_deref()),
                host,
                port,
                schema,
                description: text(&connection["description"]),
            }
        })
        .collect())
}

command! {
    pub CONNECTION_LIST = ["airflow", "connection", "list"], Read,
    "List Airflow Connections: type, host and database, never passwords",
    keywords: ["connections", "conn", "database", "host", "where", "data", "goes", "warehouse"],
    example: "airflow connection list --fields id,type,host,sql_conn",
    run: connection_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::{SqlConnection, sql_conn};
    use crate::testing::{CONFIG, airflow_with, paths};

    #[test]
    fn sql_conn_matches_the_host_and_the_database_when_both_name_one() {
        let conn = |name: &str, host: &str, database: Option<&str>| SqlConnection {
            name: name.to_owned(),
            kind: None,
            host: Some(host.to_owned()),
            port: None,
            database: database.map(str::to_owned),
        };
        let sql = [
            conn("admin", "sql.contoso.example", None),
            conn("reporting", "SQL.contoso.example", Some("reporting")),
            conn("ledger", "ora.contoso.example", None),
        ];
        let find = |host, database| sql_conn(&sql, host, None, database);
        assert_eq!(
            find(Some("sql.contoso.example"), Some("Reporting")).as_deref(),
            Some("reporting"),
            "the database decides between two on one host"
        );
        assert_eq!(
            find(Some("sql.contoso.example"), Some("staging")).as_deref(),
            Some("admin"),
            "one naming no database still matches"
        );
        assert_eq!(
            find(Some("sql.contoso.example"), None).as_deref(),
            Some("admin")
        );
        assert_eq!(
            find(Some("ora.contoso.example"), Some("x")).as_deref(),
            Some("ledger")
        );
        assert_eq!(find(Some("api.contoso.example"), None), None);
        assert_eq!(find(None, None), None);
        assert_eq!(
            sql_conn(
                &sql[1..2],
                Some("sql.contoso.example"),
                None,
                Some("staging")
            ),
            None,
            "another database never matches"
        );
        let local = [
            SqlConnection {
                kind: Some("mssql".into()),
                ..conn("ms", "localhost", Some("bench"))
            },
            SqlConnection {
                kind: Some("oracle".into()),
                ..conn("ora", "localhost", None)
            },
        ];
        assert_eq!(
            sql_conn(&local, Some("localhost"), Some(1521), None).as_deref(),
            Some("ora"),
            "two servers on one host are told apart by their ports"
        );
        assert_eq!(sql_conn(&local, Some("localhost"), Some(5432), None), None);
    }

    #[test]
    fn connection_list_names_the_sql_connection_and_never_a_credential() {
        let config = format!(
            "{CONFIG}[sql]\noracle_client_dir = \"/opt/oracle\"\n[[sql.connection]]\nname = \"reporting\"\n\
             kind = \"mssql\"\nhost = \"sql.contoso.example\"\ndatabase = \"reporting\"\nuser = \"agent\"\n\
             password_env = \"P\"\n"
        );
        let (outcome, transport) = airflow_with(
            &config,
            &["airflow", "connection", "list", "orders"],
            vec![Answer::json(&json!({"connections": [
                {"connection_id": "orders_dw", "conn_type": "mssql", "description": "The warehouse",
                 "host": "sql.contoso.example", "login": "etl_writer", "schema": "reporting",
                 "port": 1433, "password": "s3cr3t-conn-password", "extra": "{\"driver\": \"x\"}"},
                {"connection_id": "orders_api", "conn_type": "http", "description": null,
                 "host": "orders.contoso.example", "login": null, "schema": "https", "port": null,
                 "password": null, "extra": null}], "total_entries": 2}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "orders_dw", "type": "mssql", "host": "sql.contoso.example", "port": 1433,
                 "schema": "reporting", "description": "The warehouse", "sql_conn": "reporting"},
                {"id": "orders_api", "type": "http", "host": "orders.contoso.example", "schema": "https"}
            ])
        );
        for secret in ["s3cr3t", "etl_writer", "driver"] {
            assert!(!outcome.stdout.contains(secret), "{secret}");
        }
        assert_eq!(
            paths(&transport),
            ["connections?connection_id_pattern=orders&limit=50&offset=0"]
        );
    }
}
