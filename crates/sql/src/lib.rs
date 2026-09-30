//! The sql domain: queries, objects and schemas on SQL Server and Oracle,
//! ported from sql-bench. JSON out, as every agent-cli command.
//!
//! The only crate with tokio (for tiberius) and a C build (ODPI-C, which
//! loads Oracle's Instant Client at run time, so nothing else needs it).

mod catalog;
mod config;
mod connection;
mod db;
mod doctor;
mod mssql;
mod object;
mod oracle;
mod query;
mod schema;
mod split;
#[cfg(test)]
mod testing;

use agent_cli_core::Domain;

pub const DOMAIN: Domain = Domain {
    name: "sql",
    summary: "SQL Server/Oracle",
    commands: &[
        query::run::QUERY_RUN,
        query::bench::QUERY_BENCH,
        object::list::OBJECT_LIST,
        object::get::OBJECT_GET,
        schema::list::SCHEMA_LIST,
        connection::list::CONNECTION_LIST,
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
    status: doctor::status,
    doctor: doctor::doctor,
};

#[cfg(test)]
mod tests {
    use agent_cli_core::{check_layout, check_registry};

    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(check_layout(&[DOMAIN]), Vec::<String>::new());
    }
}
