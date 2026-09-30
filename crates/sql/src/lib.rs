//! The sql domain: queries, objects and schemas on SQL Server and Oracle,
//! ported from sql-bench. JSON out, as every agent-cli command.
//!
//! The only crate with tokio (for tiberius) and a C build (ODPI-C, which
//! loads Oracle's Instant Client at run time, so nothing else needs it).

mod catalog;
mod config;
mod db;
mod mssql;
mod oracle;
mod query;
mod split;

use std::time::{Duration, Instant};

use agent_cli_core::{Check, Config, Ctx, Domain, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::config::{Kind, Sql};
use crate::db::{Fetch, OnConnection, Session};

pub const DOMAIN: Domain = Domain {
    name: "sql",
    summary: "SQL Server/Oracle",
    commands: &[
        query::QUERY_RUN,
        query::QUERY_BENCH,
        catalog::OBJECT_LIST,
        catalog::OBJECT_GET,
        catalog::SCHEMA_LIST,
        CONNECTION_LIST,
    ],
    synonyms: &[
        ("table", &["object"]),
        ("tables", &["object"]),
        ("view", &["object"]),
        ("proc", &["object"]),
        ("procedure", &["object"]),
        ("stored procedure", &["object"]),
        ("function", &["object"]),
        ("package", &["object"]),
        ("sequence", &["object"]),
        ("database", &["sql"]),
        ("db", &["sql"]),
        ("statement", &["query"]),
        ("owner", &["schema"]),
        ("server", &["connection"]),
    ],
    status,
    doctor,
};

// ---------- sql connection list ----------

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

// ---------- overview and doctor ----------

fn status(config: &Config) -> String {
    if !config.has_section("sql") {
        return "sql not set up".to_owned();
    }
    match Sql::load(config) {
        Ok(sql) if sql.connections.len() == 1 => "sql 1 connection".to_owned(),
        Ok(sql) => format!("sql {} connections", sql.connections.len()),
        Err(_) => "sql config broken".to_owned(),
    }
}

/// How long doctor gives one connection.
const PROBE: Duration = Duration::from_secs(5);

/// The section parses; the Oracle client loads when an Oracle connection
/// needs it; each connection answers a trivial select within [`PROBE`].
fn doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("sql") {
        return Vec::new();
    }
    let sql = match Sql::load(ctx.config()) {
        Ok(sql) => sql,
        Err(error) => {
            return vec![Check::failed(
                "config",
                format!("{error:#}"),
                "fix [sql]; config.example.toml shows every key",
            )];
        }
    };
    let mut checks = vec![Check::ok(
        "config",
        match sql.connections.len() {
            1 => "1 connection".to_owned(),
            count => format!("{count} connections"),
        },
    )];
    let mut client = Ok(());
    if sql.connections.iter().any(|spec| spec.kind == Kind::Oracle) {
        client = oracle::init(sql.client_dir.as_deref());
        checks.push(match &client {
            Ok(()) => Check::ok(
                "oracle client",
                sql.client_dir.as_ref().map_or_else(
                    || "loaded from the system search path".to_owned(),
                    |dir| format!("loaded from {}", dir.display()),
                ),
            ),
            Err(error) => Check::failed(
                "oracle client",
                format!("{error:#}"),
                "set [sql] oracle_client_dir or AGENT_CLI_SQL_ORACLE_CLIENT_DIR",
            ),
        });
    }
    for spec in &sql.connections {
        let check = format!("connection {}", spec.name);
        if spec.kind == Kind::Oracle && client.is_err() {
            checks.push(Check::failed(
                check,
                "not checked: the Oracle client did not load",
                "fix the oracle client check first",
            ));
            continue;
        }
        let deadline = ctx.deadline().min(Instant::now() + PROBE);
        if deadline <= Instant::now() {
            checks.push(Check::failed(
                check,
                "not checked: --timeout ran out",
                "agent-cli doctor sql --timeout 120",
            ));
            continue;
        }
        let probe = match spec.kind {
            Kind::Mssql => "select 1",
            Kind::Oracle => "select 1 from dual",
        };
        let started = Instant::now();
        let op = OnConnection {
            spec,
            client_dir: sql.client_dir.as_deref(),
            deadline,
            plan: json!({"conn": spec.name, "read": "probe"}),
            writes: false,
            work: |session: &mut Session| session.run(probe, Fetch::ALL, deadline),
        };
        checks.push(match ctx.read(op) {
            Ok(_) => Check::ok(
                check,
                format!(
                    "{}:{} answered `{probe}` in {} ms",
                    spec.host,
                    spec.port,
                    started.elapsed().as_millis()
                ),
            ),
            Err(error) => Check::failed(
                check,
                format!("{error:#}"),
                format!(
                    "check host, port, user and password of {:?} in [sql]",
                    spec.name
                ),
            ),
        });
    }
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{FakeTransport, Outcome, run};
    use agent_cli_core::{Setup, check_registry};

    use super::*;

    /// Nothing listens on port 9 (discard), so a test that did connect would
    /// fail fast rather than reach a database.
    const CONFIG: &str = r#"
[[sql.connection]]
name = "ms"
kind = "mssql"
host = "127.0.0.1"
port = 9
database = "bench"
user = "sa"
password = "s3cret-literal"

[[sql.connection]]
name = "ora"
kind = "oracle"
host = "127.0.0.1"
port = 9
service = "FREEPDB1"
user = "bench"
password_cmd = "pass show contoso/ora"
read_only = true

[[sql.connection]]
name = "env"
kind = "mssql"
host = "127.0.0.1"
port = 9
database = "bench"
user = "u"
password_env = "CONTOSO_DB_PASSWORD"
"#;

    fn sql(argv: &[&str], setup: Setup) -> Outcome {
        run(&[DOMAIN], argv, setup)
    }

    fn setup() -> Setup {
        Setup::fake(FakeTransport::default()).with_config(CONFIG)
    }

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
    }

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

    #[test]
    fn a_write_under_dry_run_prints_each_statement_and_its_class_and_connects_to_nothing() {
        for argv in [
            &[
                "sql",
                "query",
                "run",
                "--conn",
                "ms",
                "select 1\ngo\ndelete from t",
            ][..],
            &[
                "sql",
                "query",
                "bench",
                "--conn",
                "ms",
                "--runs",
                "3",
                "update t set a = 1",
            ][..],
        ] {
            let mut argv = argv.to_vec();
            argv.push("--dry-run");
            let outcome = sql(&argv, setup());
            assert_eq!(outcome.code, 0, "{outcome:?}");
            let printed = outcome.json();
            assert_eq!(printed["dry_run"], true, "{outcome:?}");
            let plan = &printed["would"][0];
            assert_eq!(plan["conn"], "ms");
            let last = plan["statements"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()
                .clone();
            assert_eq!(last["writes"], true, "{plan}");
        }
        let outcome = sql(
            &[
                "sql",
                "query",
                "run",
                "--conn",
                "ms",
                "select 1\ngo\ndelete from t",
                "--dry-run",
            ],
            setup(),
        );
        assert_eq!(
            outcome.json()["would"][0]["statements"],
            json!([
                {"sql": "select 1", "writes": false},
                {"sql": "delete from t", "writes": true}
            ])
        );
    }

    #[test]
    fn sql_comes_as_the_argument_piped_with_a_dash_or_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fix.sql");
        std::fs::write(&file, "select 1\r\ngo\r\ndelete from t\r\n").unwrap();
        let file = file.to_str().unwrap();
        for (argv, stdin) in [
            (
                &["sql", "query", "run", "--conn", "ms", "--sql-file", file][..],
                "",
            ),
            (
                &["sql", "query", "bench", "--conn", "ms", "--sql-file", file][..],
                "",
            ),
            (
                &["sql", "query", "run", "--conn", "ms", "-"][..],
                "select 1\ngo\ndelete from t\n",
            ),
        ] {
            let mut argv = argv.to_vec();
            argv.push("--dry-run");
            let outcome = sql(&argv, setup().with_stdin(stdin));
            assert_eq!(outcome.code, 0, "{outcome:?}");
            assert_eq!(
                outcome.json()["would"][0]["statements"],
                json!([
                    {"sql": "select 1", "writes": false},
                    {"sql": "delete from t", "writes": true}
                ])
            );
        }
        for argv in [
            &["sql", "query", "run", "--conn", "ms"][..],
            &[
                "sql",
                "query",
                "run",
                "--conn",
                "ms",
                "select 1",
                "--sql-file",
                file,
            ][..],
            &["sql", "query", "run", "--conn", "ms", "-"][..],
        ] {
            let outcome = sql(argv, setup());
            assert_eq!(outcome.code, 2, "{outcome:?}");
        }
    }

    #[test]
    fn bench_takes_max_rows_as_query_run_does() {
        let outcome = sql(
            &[
                "sql",
                "query",
                "bench",
                "--conn",
                "ms",
                "--max-rows",
                "10",
                "--runs",
                "2",
                "update t set a = 1",
                "--dry-run",
            ],
            setup(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["would"][0]["runs"], 2);
        let outcome = sql(
            &[
                "sql",
                "query",
                "bench",
                "--conn",
                "ms",
                "--max-rows",
                "0",
                "select 1",
            ],
            setup(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_write_needs_yes_and_read_only_mode_refuses_it() {
        let outcome = sql(
            &["sql", "query", "run", "--conn", "ms", "drop table t"],
            setup(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stdout.is_empty());
        assert!(outcome.stderr.contains("--yes"), "{}", outcome.stderr);

        for verb in ["run", "bench"] {
            let outcome = sql(
                &[
                    "sql",
                    "query",
                    verb,
                    "--conn",
                    "ms",
                    "insert into t values (1)",
                    "--yes",
                ],
                setup().read_only(),
            );
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(
                outcome.stderr.contains("AGENT_CLI_READ_ONLY"),
                "{}",
                outcome.stderr
            );
        }
    }

    #[test]
    fn a_read_only_connection_refuses_a_write_even_with_yes_or_dry_run() {
        for extra in ["--yes", "--dry-run"] {
            let outcome = sql(
                &[
                    "sql",
                    "query",
                    "run",
                    "--conn",
                    "ora",
                    "select 1 from t for update",
                    extra,
                ],
                setup(),
            );
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(
                outcome.stderr.contains("connection \"ora\" is read_only"),
                "{}",
                outcome.stderr
            );
            assert!(outcome.stderr.contains("hint: "), "{}", outcome.stderr);
        }
    }

    #[test]
    fn a_read_is_never_refused_by_read_only_mode_and_goes_to_the_server() {
        let outcome = sql(
            &[
                "sql",
                "query",
                "run",
                "--conn",
                "ms",
                "select 1",
                "--timeout",
                "5",
            ],
            setup().read_only(),
        );
        assert_eq!(outcome.code, 1, "port 9 does not answer: {outcome:?}");
        assert!(!outcome.stderr.contains("READ_ONLY"), "{}", outcome.stderr);
        assert!(
            outcome
                .stderr
                .starts_with("error: cannot connect to 127.0.0.1:9"),
            "{}",
            outcome.stderr
        );
        assert!(!outcome.stderr.contains("s3cret-literal"));
    }

    #[test]
    fn nothing_to_run_and_an_unknown_connection_are_usage_errors() {
        let outcome = sql(
            &["sql", "query", "run", "--conn", "ms", "-- only a note"],
            setup(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        let outcome = sql(&["sql", "schema", "list", "--conn", "nope"], setup());
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("--conn takes one of: ms, ora, env"),
            "{}",
            outcome.stderr
        );
        let outcome = sql(&["sql", "schema", "list"], setup());
        assert_eq!(
            outcome.code, 2,
            "three connections leave the choice open: {outcome:?}"
        );
        assert!(
            outcome.stderr.contains(
                "more than one connection is configured; name one with --conn: ms, ora, env"
            ),
            "{}",
            outcome.stderr
        );
        let outcome = sql(
            &["sql", "object", "get", "--conn", "ms", "customers"],
            setup(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("expected SCHEMA.NAME"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn the_overview_counts_connections_and_a_broken_section_says_so() {
        let config = |toml: Option<&str>| Config::parse("c.toml", toml, Vec::new());
        assert_eq!(status(&config(Some(CONFIG))), "sql 3 connections");
        assert_eq!(status(&config(None)), "sql not set up");
        assert_eq!(
            status(&config(Some("[sql]\nbogus = 1\n"))),
            "sql config broken"
        );
    }
}
