//! `airflow connection diff`: two instances' Connections by id: those on
//! one side only, type, host, port, schema and description; the login and
//! each key of `extra` compared in memory and printed only as `same:
//! true|false` (none where Airflow masks the value). Airflow never returns
//! a password, so passwords are not compared.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, Rules, Secret, command};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::Client;
use crate::diff::{Instances, Read, unmasked};

/// The most Connections read from each instance.
const MOST: usize = 1000;

fn connection_diff(ctx: &Ctx, args: Instances) -> Result<Diff> {
    ctx.note("[Airflow never returns a connection's password: passwords are not compared]");
    args.compare(ctx, &Rules::PLAIN, connections)
}

fn connections(client: &Client) -> Result<Read> {
    let (items, _) = client.list("connections", "", "connections", MOST)?;
    let mut rows = BTreeMap::new();
    let mut values = BTreeMap::new();
    for item in items {
        let Some(id) = item["connection_id"].as_str().map(str::to_owned) else {
            continue;
        };
        if let Some(login) = item["login"].as_str() {
            values.insert(format!("{id}.login"), Some(Secret::new(login)));
        }
        // `extra` is a JSON object in a string; its values may be secrets.
        let extra: Value = item["extra"]
            .as_str()
            .and_then(|extra| serde_json::from_str(extra).ok())
            .unwrap_or_default();
        for (key, value) in extra.as_object().into_iter().flatten() {
            values.insert(format!("{id}.extra.{key}"), unmasked(value));
        }
        rows.insert(
            id,
            json!({"type": item["conn_type"], "host": item["host"], "port": item["port"],
                "schema": item["schema"], "description": item["description"]}),
        );
    }
    Ok((rows, values))
}

command! {
    pub CONNECTION_DIFF = ["airflow", "connection", "diff"], Read,
    "Compare two instances' Connections: missing ones, host, port, login same or not",
    keywords: ["staging", "missing", "same", "host", "extra"],
    example: "airflow connection diff qa prod",
    run: connection_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{TWO, airflow_with};

    #[test]
    fn connections_differ_by_host_with_logins_and_extras_never_printed() {
        let one = |host: &str, login: &str, container: &str| {
            Answer::json(&json!({"connections": [
                {"connection_id": "reporting_dw", "conn_type": "mssql", "host": host,
                    "login": login, "schema": "reporting", "port": 1433, "password": "***",
                    "extra": format!("{{\"container\": \"{container}\", \"token\": \"***\"}}")},
            ], "total_entries": 1}))
        };
        let (outcome, _) = airflow_with(
            TWO,
            &["airflow", "connection", "diff", "qa", "prod"],
            vec![
                one("sql-qa.contoso.example", "etl_writer_qa", "logs"),
                one("sql.contoso.example", "etl_writer_prod", "logs"),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["changes"],
            json!([
                {"path": "reporting_dw.host", "left": "sql-qa.contoso.example", "right": "sql.contoso.example"},
                {"path": "reporting_dw.extra.token", "redacted": true},
                {"path": "reporting_dw.login", "redacted": true, "same": false},
            ])
        );
        assert!(!outcome.stdout.contains("etl_writer"), "{}", outcome.stdout);
    }
}
