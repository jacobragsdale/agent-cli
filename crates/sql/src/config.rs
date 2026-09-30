//! The `[sql]` section: the connections, and where the Oracle Instant Client
//! is.
//!
//! ```toml
//! [sql]
//! oracle_client_dir = "~/.local/opt/oracle/instantclient_23_26"  # optional
//!
//! [[sql.connection]]
//! name = "local-mssql"   # unique; what --conn takes
//! kind = "mssql"         # or "oracle"
//! host = "localhost"
//! port = 1433            # left out: 1433 for mssql, 1521 for oracle
//! database = "contoso"   # mssql only
//! user = "app"
//! password_env = "VAR"   # or password = "...", or password_cmd = "pass show x"
//! trust_cert = true      # mssql only, default false
//! encrypt = true         # mssql only, default true
//! read_only = true       # refuse every write on this connection, default false
//! ```
//!
//! Everything is checked when a sql command reads the section, except the
//! password: that is resolved only when a connection opens, so listing ten
//! connections never runs ten `password_cmd`s.

use std::path::PathBuf;

use agent_cli_core::{Config, Credential, Ctx, Failure, Secret, pick};
use anyhow::Result;
use serde::Deserialize;

/// Which driver a connection is opened with, and which dialect its SQL is
/// split and classified in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mssql,
    Oracle,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mssql => "mssql",
            Self::Oracle => "oracle",
        }
    }

    /// What the vendor's installer listens on, for a connection with no port.
    fn default_port(self) -> u16 {
        match self {
            Self::Mssql => 1433,
            Self::Oracle => 1521,
        }
    }
}

/// One `[[sql.connection]]`, checked, with the defaults filled in.
#[derive(Clone, Debug)]
pub struct Connection {
    pub name: String,
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    /// Always set for mssql, never for oracle.
    pub database: Option<String>,
    /// Always set for oracle, never for mssql.
    pub service: Option<String>,
    pub user: String,
    /// `password`, `password_env` or `password_cmd`: core's [`Credential`].
    pub password: Option<Credential>,
    pub trust_cert: bool,
    pub encrypt: bool,
    pub read_only: bool,
}

/// The whole `[sql]` section, checked.
#[derive(Debug, Default)]
pub struct Sql {
    pub connections: Vec<Connection>,
    /// `oracle_client_dir`, which `AGENT_CLI_SQL_ORACLE_CLIENT_DIR` overrides
    /// the way it overrides any key; `None` leaves ODPI-C to its own search
    /// (`LD_LIBRARY_PATH`, the run path, `$ORACLE_HOME`).
    pub client_dir: Option<PathBuf>,
    path: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    #[serde(default)]
    oracle_client_dir: Option<String>,
    #[serde(default)]
    connection: Vec<Raw>,
}

/// A connection as written. `kind` is a string so a wrong one is reported
/// against the connection's name.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    name: String,
    kind: String,
    host: String,
    port: Option<u16>,
    database: Option<String>,
    service: Option<String>,
    user: String,
    password: Option<String>,
    password_env: Option<String>,
    password_cmd: Option<String>,
    trust_cert: Option<bool>,
    encrypt: Option<bool>,
    #[serde(default)]
    read_only: bool,
}

impl Sql {
    /// The `[sql]` section of `config`; exit 3 naming the file when it is wrong.
    pub fn load(config: &Config) -> Result<Self> {
        let section: Section = config.section("sql")?;
        let path = config.path().to_path_buf();
        let fix = |message: String| -> anyhow::Error {
            Failure::setup(format!("[sql] in {}: {message}", path.display()))
                .hint("fix it; config.example.toml shows every key")
                .into()
        };
        let mut connections: Vec<Connection> = Vec::with_capacity(section.connection.len());
        for raw in section.connection {
            let name = raw.name.clone();
            if connections.iter().any(|earlier| earlier.name == name) {
                return Err(fix(format!("connection {name:?} is named twice")));
            }
            connections.push(
                raw.check()
                    .map_err(|why| fix(format!("connection {name:?}: {why}")))?,
            );
        }
        Ok(Self {
            connections,
            client_dir: section
                .oracle_client_dir
                .filter(|dir| !dir.trim().is_empty())
                .map(|dir| expand_home(&dir)),
            path,
        })
    }

    /// The connection `--conn` names, or the only one when it names none
    /// (core's rule for every scope flag): exit 2 listing the ones there are,
    /// or exit 3 when there are none at all.
    pub fn connection(&self, name: Option<&str>) -> Result<&Connection> {
        if self.connections.is_empty() {
            return Err(Failure::setup(format!(
                "no [[sql.connection]] in {}",
                self.path.display()
            ))
            .hint("add one; config.example.toml shows the keys")
            .into());
        }
        pick("connection", "--conn", name, &self.connections, |spec| {
            &spec.name
        })
        .map_err(|failure| failure.hint("agent-cli sql connection list").into())
    }
}

impl Raw {
    fn check(self) -> std::result::Result<Connection, String> {
        let kind = match self.kind.as_str() {
            "mssql" => Kind::Mssql,
            "oracle" => Kind::Oracle,
            other => return Err(format!("kind {other:?} is not \"mssql\" or \"oracle\"")),
        };
        if self.name.trim().is_empty() {
            return Err("name is empty; --conn picks a connection by it".to_owned());
        }
        if self.host.trim().is_empty() {
            return Err("host is empty".to_owned());
        }
        // A key the other kind reads would look like it did something.
        let foreign = match kind {
            Kind::Mssql => [("service", self.service.is_some())].to_vec(),
            Kind::Oracle => [
                ("database", self.database.is_some()),
                ("trust_cert", self.trust_cert.is_some()),
                ("encrypt", self.encrypt.is_some()),
            ]
            .to_vec(),
        };
        if let Some((key, _)) = foreign.iter().find(|(_, set)| *set) {
            return Err(format!("{key} is not a setting of kind {:?}", self.kind));
        }
        match kind {
            Kind::Mssql if self.database.is_none() => {
                return Err("kind \"mssql\" needs a database".to_owned());
            }
            Kind::Oracle if self.service.is_none() => {
                return Err("kind \"oracle\" needs a service".to_owned());
            }
            _ => {}
        }
        let port = match self.port {
            Some(0) => {
                return Err(format!(
                    "port 0 is not a port; leave it out for {}",
                    kind.default_port()
                ));
            }
            Some(port) => port,
            None => kind.default_port(),
        };
        let password = Credential::from_keys(
            "password",
            self.password,
            self.password_env,
            self.password_cmd,
        )?;
        Ok(Connection {
            name: self.name,
            kind,
            host: self.host,
            port,
            database: self.database,
            service: self.service,
            user: self.user,
            password,
            trust_cert: self.trust_cert.unwrap_or(false),
            encrypt: self.encrypt.unwrap_or(true),
            read_only: self.read_only,
        })
    }
}

impl Connection {
    /// The password, resolved now that the connection is opening: a
    /// `password_cmd` runs under the command's deadline like any other child
    /// process. None is an empty one, which is what a trusted login sends.
    pub fn password(&self, ctx: &Ctx) -> Result<Secret> {
        match &self.password {
            None => Ok(Secret::new(String::new())),
            Some(credential) => credential
                .resolve(ctx)
                .map_err(|error| error.context(format!("connection {:?}", self.name))),
        }
    }
}

/// A leading `~/` is the home directory; nothing else is expanded.
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(toml: &str) -> Result<Sql> {
        Sql::load(&Config::parse("/cfg/config.toml", Some(toml), Vec::new()))
    }

    fn failure(toml: &str) -> String {
        format!("{:#}", load(toml).unwrap_err())
    }

    const MSSQL: &str = r#"
[[sql.connection]]
name = "local-mssql"
kind = "mssql"
host = "localhost"
database = "bench"
user = "sa"
password = "hunter2hunter2"
trust_cert = true
"#;

    #[test]
    fn a_connection_keeps_what_the_file_says_and_defaults_the_rest() {
        let sql = load(MSSQL).unwrap();
        let spec = sql.connection(Some("local-mssql")).unwrap();
        assert_eq!((spec.kind, spec.port), (Kind::Mssql, 1433));
        assert!(spec.trust_cert && spec.encrypt && !spec.read_only);
        assert_eq!(sql.client_dir, None);
        let oracle = load(
            "[sql]\noracle_client_dir = \"/opt/ic\"\n[[sql.connection]]\nname = \"o\"\nkind = \"oracle\"\nhost = \"h\"\nservice = \"FREEPDB1\"\nuser = \"u\"\nread_only = true\n",
        )
        .unwrap();
        let spec = oracle.connection(Some("o")).unwrap();
        assert_eq!(
            (spec.kind, spec.port, spec.read_only),
            (Kind::Oracle, 1521, true)
        );
        assert_eq!(oracle.client_dir, Some(PathBuf::from("/opt/ic")));
    }

    #[test]
    fn the_environment_overrides_the_client_directory() {
        let config = Config::parse(
            "c.toml",
            Some("[sql]\noracle_client_dir = \"/opt/ic\"\n"),
            vec![(
                "AGENT_CLI_SQL_ORACLE_CLIENT_DIR".to_owned(),
                "/elsewhere/ic".to_owned(),
            )],
        );
        assert_eq!(
            Sql::load(&config).unwrap().client_dir,
            Some(PathBuf::from("/elsewhere/ic"))
        );
        let tilde = load("[sql]\noracle_client_dir = \"~/ic\"\n").unwrap();
        assert!(tilde.client_dir.unwrap().ends_with("ic") && std::env::var_os("HOME").is_some());
    }

    #[test]
    fn every_mistake_names_the_connection_and_is_exit_three() {
        let base = |extra: &str| {
            format!(
                "[[sql.connection]]\nname = \"ledger\"\nkind = \"mssql\"\nhost = \"h\"\nuser = \"u\"\n{extra}\n"
            )
        };
        let cases = [
            (base("database = \"d\"\npasword = \"x\""), "unknown field `pasword`"),
            (base(""), "connection \"ledger\": kind \"mssql\" needs a database"),
            (
                base("database = \"d\"\nservice = \"s\""),
                "connection \"ledger\": service is not a setting of kind \"mssql\"",
            ),
            (
                base("database = \"d\"\nport = 0"),
                "connection \"ledger\": port 0 is not a port",
            ),
            (
                base("database = \"d\"\npassword = \"a\"\npassword_env = \"B\""),
                "give one of password, password_env and password_cmd",
            ),
            (
                format!("{}{}", base("database = \"d\""), base("database = \"d\"")),
                "connection \"ledger\" is named twice",
            ),
            (
                "[[sql.connection]]\nname = \"x\"\nkind = \"postgres\"\nhost = \"h\"\nuser = \"u\"\n"
                    .to_owned(),
                "kind \"postgres\" is not \"mssql\" or \"oracle\"",
            ),
        ];
        for (toml, want) in cases {
            let error = load(&toml).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains(want), "{message}");
            let exit = error
                .chain()
                .find_map(|cause| cause.downcast_ref::<Failure>())
                .map(|failure| failure.exit);
            assert_eq!(exit, Some(agent_cli_core::Exit::Setup), "{message}");
        }
        assert!(failure("[sql]\nnope = 1\n").contains("unknown field `nope`"));
    }

    #[test]
    fn an_unknown_connection_lists_the_ones_there_are_and_the_only_one_is_the_default() {
        let sql = load(MSSQL).unwrap();
        assert_eq!(sql.connection(None).unwrap().name, "local-mssql");
        let message = format!("{:#}", sql.connection(Some("prod")).unwrap_err());
        assert_eq!(
            message,
            "no connection \"prod\"; --conn takes one of: local-mssql"
        );
        let two = load(&format!("{MSSQL}{}", MSSQL.replace("local-mssql", "other"))).unwrap();
        let message = format!("{:#}", two.connection(None).unwrap_err());
        assert_eq!(
            message,
            "more than one connection is configured; name one with --conn: local-mssql, other"
        );
        let none = load("").unwrap();
        assert!(
            format!("{:#}", none.connection(Some("x")).unwrap_err())
                .starts_with("no [[sql.connection]] in /cfg/config.toml")
        );
    }

    #[test]
    fn a_password_comes_from_the_file_a_variable_or_a_command() {
        use agent_cli_core::testing::{FakeTransport, ctx};
        let ctx = ctx(agent_cli_core::Setup::fake(FakeTransport::default())
            .with_env("SQL_TEST_PASSWORD", "from-env-1"));
        let sql = load(MSSQL).unwrap();
        let spec = sql.connection(Some("local-mssql")).unwrap();
        assert_eq!(spec.password(&ctx).unwrap().expose(), "hunter2hunter2");

        let with = |source: (Option<&str>, Option<&str>)| {
            let mut spec = spec.clone();
            spec.password = Credential::from_keys(
                "password",
                None,
                source.0.map(str::to_owned),
                source.1.map(str::to_owned),
            )
            .unwrap();
            spec
        };
        assert_eq!(
            with((None, Some("printf 's3cret\\n'")))
                .password(&ctx)
                .unwrap()
                .expose(),
            "s3cret"
        );
        assert_eq!(
            with((Some("SQL_TEST_PASSWORD"), None))
                .password(&ctx)
                .unwrap()
                .expose(),
            "from-env-1"
        );
        let message = format!(
            "{:#}",
            with((None, Some("echo nope >&2; exit 3")))
                .password(&ctx)
                .unwrap_err()
        );
        assert!(
            message.contains("password_cmd failed") && message.contains("nope"),
            "{message}"
        );
        let message = format!(
            "{:#}",
            with((Some("UNSET_VAR"), None)).password(&ctx).unwrap_err()
        );
        assert_eq!(
            message,
            "connection \"local-mssql\": password_env UNSET_VAR is not set"
        );
    }

    #[test]
    fn nothing_to_run_and_an_unknown_connection_are_usage_errors() {
        use crate::testing::{setup, sql};

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
}
