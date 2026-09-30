//! The overview's line for sql, and `agent-cli doctor sql`.

use std::time::{Duration, Instant};

use agent_cli_core::{Check, Config, Ctx};
use serde_json::json;

use crate::config::{Kind, Sql};
use crate::db::{Fetch, OnConnection, Session};
use crate::oracle;

pub(crate) fn status(config: &Config) -> String {
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
pub(crate) fn doctor(ctx: &Ctx) -> Vec<Check> {
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

    use super::*;
    use crate::testing::CONFIG;

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
