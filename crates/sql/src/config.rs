//! The `[sql]` section: the connections, and where the Oracle Instant Client
//! is.
//!
//! ```toml
//! [sql]
//! oracle_client_dir = "~/.local/opt/oracle/instantclient_23_26"  # optional
//!
//! [[sql.connection]]
//! name = "local-mssql"   # unique; what --conn takes
//! kind = "mssql"         # or "oracle" or "postgres"
//! host = "localhost"
//! port = 1433            # left out: 1433 mssql, 1521 oracle, 5432 postgres
//! database = "contoso"   # mssql and postgres
//! user = "app"
//! password_env = "VAR"   # or password = "...", password_cmd = "pass show x",
//!                        # or password_kv = "VAULT/SECRET" (Key Vault)
//! read_only = true       # refuse every change here; overrides [sql] read_only
//! trust_cert = true      # mssql and postgres, default false
//! encrypt = true         # mssql and postgres, default true: TLS required
//!
//! [[sql.connection]]     # Synapse serverless, Azure SQL: Entra ID sign-in
//! name = "synapse"
//! kind = "mssql"
//! host = "contoso-ws-ondemand.sql.azuresynapse.net"
//! database = "master"
//! auth = "az"            # your az login's token, no user or password
//!
//! [[sql.connection]]     # Azure Database for PostgreSQL Flexible Server
//! name = "events"
//! kind = "postgres"
//! host = "contoso-events.postgres.database.azure.com"
//! database = "events"
//! user = "contoso-pg-readers"  # the Entra user or group the server knows
//! auth = "az"            # your az login's token is the password
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
    Postgres,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mssql => "mssql",
            Self::Oracle => "oracle",
            Self::Postgres => "postgres",
        }
    }

    /// What the vendor's installer listens on, for a connection with no port.
    fn default_port(self) -> u16 {
        match self {
            Self::Mssql => 1433,
            Self::Oracle => 1521,
            Self::Postgres => 5432,
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
    /// Always set for mssql and postgres, never for oracle.
    pub database: Option<String>,
    /// Always set for oracle, never for the others.
    pub service: Option<String>,
    /// Empty for mssql's `auth = "az"`; Postgres names the Entra principal.
    pub user: String,
    /// `password`, `password_env`, `password_cmd` or `password_kv`: core's
    /// [`Credential`].
    pub password: Option<Credential>,
    /// The entry's own `read_only`; `None` leaves it to `[sql] read_only`,
    /// which core reads.
    pub read_only: Option<bool>,
    /// `auth = "az"`: sign in with the az login's Entra ID token.
    pub az: bool,
    /// Any certificate is taken (mssql, postgres).
    pub trust_cert: bool,
    /// TLS is required (mssql, postgres); off, the connection is plain.
    pub encrypt: bool,
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
    user: Option<String>,
    auth: Option<String>,
    password: Option<String>,
    password_env: Option<String>,
    password_cmd: Option<String>,
    password_kv: Option<String>,
    /// Refuses every change on this connection; overrides `[sql] read_only`.
    read_only: Option<bool>,
    trust_cert: Option<bool>,
    encrypt: Option<bool>,
}

impl Sql {
    /// The `[sql]` section of `config`; exit 3 naming the file when it is wrong.
    pub fn load(config: &Config) -> Result<Self> {
        let section: Section = config.section("sql")?;
        let path = config.path().to_path_buf();
        let fix = |message: String| -> anyhow::Error {
            Failure::setup(format!("[sql] in {}: {message}", path.display()))
                .hint("fix it; `agent-cli config example sql` shows every key")
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
    /// or exit 3 when there are none at all. Every command picks its
    /// connection here, so this is where it enters the connection's scope:
    /// from then on `ctx.write` refuses a change on a read-only one.
    pub fn connection(&self, ctx: &Ctx, name: Option<&str>) -> Result<&Connection> {
        if self.connections.is_empty() {
            return Err(Failure::setup(format!(
                "no [[sql.connection]] in {}",
                self.path.display()
            ))
            .hint("agent-cli config example sql  (prints one to paste)")
            .into());
        }
        let spec = pick("connection", "--conn", name, &self.connections, |spec| {
            &spec.name
        })
        .map_err(|failure| failure.hint("agent-cli sql connection list"))?;
        spec.enter(ctx);
        Ok(spec)
    }
}

impl Raw {
    fn check(self) -> std::result::Result<Connection, String> {
        let kind = match self.kind.as_str() {
            "mssql" => Kind::Mssql,
            "oracle" => Kind::Oracle,
            "postgres" => Kind::Postgres,
            other => {
                return Err(format!(
                    "kind {other:?} is not \"mssql\", \"oracle\" or \"postgres\""
                ));
            }
        };
        if self.name.trim().is_empty() {
            return Err("name is empty; --conn picks a connection by it".to_owned());
        }
        if self.host.trim().is_empty() {
            return Err("host is empty".to_owned());
        }
        // A key the other kind reads would look like it did something.
        let foreign = match kind {
            Kind::Mssql | Kind::Postgres => [("service", self.service.is_some())].to_vec(),
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
            Kind::Mssql | Kind::Postgres if self.database.is_none() => {
                return Err(format!("kind {:?} needs a database", kind.as_str()));
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
        let az = match self.auth.as_deref() {
            None | Some("password") => false,
            Some("az") if kind != Kind::Oracle => true,
            Some("az") => {
                return Err("auth \"az\" is for kinds \"mssql\" and \"postgres\"".to_owned());
            }
            Some(other) => return Err(format!("auth {other:?} is not \"password\" or \"az\"")),
        };
        let password = Credential::from_keys(
            "password",
            self.password,
            self.password_env,
            self.password_cmd,
            self.password_kv,
        )?;
        let user = match (az, self.user) {
            // Flexible Server signs in as the role named for the principal,
            // which the token does not choose: a user's or a group's name.
            (true, Some(user)) if kind == Kind::Postgres && password.is_none() => user,
            (true, _) if kind == Kind::Postgres => {
                return Err(
                    "auth \"az\" on postgres takes user (the Entra user or group the server \
                     knows) and no password"
                        .to_owned(),
                );
            }
            (true, None) if password.is_none() => String::new(),
            (true, _) => {
                return Err(
                    "auth \"az\" signs in as your az login: leave out user and password".to_owned(),
                );
            }
            (false, Some(user)) => user,
            (false, None) => return Err("user is missing (or set auth = \"az\")".to_owned()),
        };
        Ok(Connection {
            name: self.name,
            kind,
            host: self.host,
            port,
            database: self.database,
            service: self.service,
            user,
            password,
            read_only: self.read_only,
            az,
            trust_cert: self.trust_cert.unwrap_or(false),
            encrypt: self.encrypt.unwrap_or(true),
        })
    }
}

impl Connection {
    /// Makes this the connection `ctx.write` checks `read_only` against and
    /// `ctx.read_only` answers for. A diff picks both sides before reading
    /// either, so each side's read enters its own again.
    pub fn enter(&self, ctx: &Ctx) {
        ctx.enter_scope("sql connection", &self.name, self.read_only);
    }

    /// What the connection signs in with, resolved now that it is opening:
    /// the password (a `password_cmd` runs under the command's deadline like
    /// any other child process; none is an empty one, which is what a trusted
    /// login sends), or for `auth = "az"` the az login's token for Azure SQL,
    /// which Synapse takes too, or for Azure Database for PostgreSQL.
    pub fn password(&self, ctx: &Ctx) -> Result<Secret> {
        if self.az {
            let audience = match self.kind {
                Kind::Postgres => AZURE_POSTGRES,
                Kind::Mssql | Kind::Oracle => AZURE_SQL,
            };
            return ctx
                .az_token(audience, false)
                .map_err(|error| error.context(format!("connection {:?}", self.name)));
        }
        match &self.password {
            None => Ok(Secret::new(String::new())),
            Some(credential) => credential
                .resolve(ctx)
                .map_err(|error| error.context(format!("connection {:?}", self.name))),
        }
    }

    /// A new password after the server refused this one, when there is one
    /// to try: a `password_kv` whose secret changed in Key Vault since it was
    /// kept (a rotation). `None` means the refusal stands.
    pub fn refreshed(&self, ctx: &Ctx) -> Result<Option<Secret>> {
        match &self.password {
            Some(credential) if !self.az => credential
                .refresh(ctx)
                .map_err(|error| error.context(format!("connection {:?}", self.name))),
            _ => Ok(None),
        }
    }
}

/// The audience of an Entra ID token for Azure SQL and Synapse SQL.
pub const AZURE_SQL: &str = "https://database.windows.net/";

/// The audience of an Entra ID token for Azure Database for PostgreSQL
/// (`az account get-access-token --resource-type oss-rdbms`).
pub const AZURE_POSTGRES: &str = "https://ossrdbms-aad.database.windows.net";

/// A leading `~/` is the home directory; nothing else is expanded.
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(toml: &str) -> Result<Sql> {
        Sql::load(&Config::parse("/cfg/config.toml", Some(toml), Vec::new()))
    }

    fn ctx() -> Ctx {
        agent_cli_core::testing::ctx(agent_cli_core::Setup::fake(
            agent_cli_core::testing::FakeTransport::default(),
        ))
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
        let spec = sql.connection(&ctx(), Some("local-mssql")).unwrap();
        assert_eq!((spec.kind, spec.port), (Kind::Mssql, 1433));
        assert!(spec.trust_cert && spec.encrypt);
        assert_eq!(sql.client_dir, None);
        let oracle = load(
            "[sql]\noracle_client_dir = \"/opt/ic\"\n[[sql.connection]]\nname = \"o\"\nkind = \"oracle\"\nhost = \"h\"\nservice = \"FREEPDB1\"\nuser = \"u\"\n",
        )
        .unwrap();
        let spec = oracle.connection(&ctx(), Some("o")).unwrap();
        assert_eq!((spec.kind, spec.port), (Kind::Oracle, 1521));
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
        assert_eq!(
            tilde.client_dir,
            std::env::home_dir().map(|home| home.join("ic"))
        );
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
                "give one of password, password_env, password_cmd and password_kv",
            ),
            (
                format!("{}{}", base("database = \"d\""), base("database = \"d\"")),
                "connection \"ledger\" is named twice",
            ),
            (
                "[[sql.connection]]\nname = \"x\"\nkind = \"db2\"\nhost = \"h\"\nuser = \"u\"\n"
                    .to_owned(),
                "kind \"db2\" is not \"mssql\", \"oracle\" or \"postgres\"",
            ),
            (
                "[[sql.connection]]\nname = \"pg\"\nkind = \"postgres\"\nhost = \"h\"\nuser = \"u\"\n"
                    .to_owned(),
                "connection \"pg\": kind \"postgres\" needs a database",
            ),
            (
                "[[sql.connection]]\nname = \"pg\"\nkind = \"postgres\"\nhost = \"h\"\ndatabase = \"d\"\nuser = \"u\"\nservice = \"s\"\n"
                    .to_owned(),
                "connection \"pg\": service is not a setting of kind \"postgres\"",
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
    fn auth_az_signs_in_with_the_az_token_for_azure_sql_and_takes_no_user_or_password() {
        let synapse = |extra: &str| {
            format!(
                "[[sql.connection]]\nname = \"synapse\"\nkind = \"mssql\"\nhost = \"contoso-ws-ondemand.sql.azuresynapse.net\"\ndatabase = \"master\"\nauth = \"az\"\n{extra}\n"
            )
        };
        let sql = load(&synapse("")).unwrap();
        let spec = sql.connection(&ctx(), None).unwrap();
        assert!(spec.az && spec.encrypt && spec.user.is_empty());
        let ctx = agent_cli_core::testing::ctx(agent_cli_core::Setup::fake(
            agent_cli_core::testing::FakeTransport::default(),
        ));
        assert_eq!(
            spec.password(&ctx).unwrap().expose(),
            "token@https://database.windows.net/"
        );
        for (extra, want) in [
            ("user = \"u\"", "leave out user and password"),
            ("password_env = \"P\"", "leave out user and password"),
        ] {
            assert!(failure(&synapse(extra)).contains(want), "{extra}");
        }
        assert!(failure(&MSSQL.replace("user = \"sa\"\n", "")).contains("user is missing"));
        assert!(
            failure(&format!("{MSSQL}auth = \"kerberos\"\n"))
                .contains("not \"password\" or \"az\"")
        );
        let oracle = "[[sql.connection]]\nname = \"o\"\nkind = \"oracle\"\nhost = \"h\"\nservice = \"S\"\nauth = \"az\"\n";
        assert!(failure(oracle).contains("for kinds \"mssql\" and \"postgres\""));
    }

    const FLEXIBLE: &str = "[[sql.connection]]\nname = \"events\"\nkind = \"postgres\"\nhost = \"contoso-events.postgres.database.azure.com\"\ndatabase = \"events\"\nauth = \"az\"\n";

    #[test]
    fn a_postgres_connection_defaults_to_5432_with_tls_required() {
        let sql = load(
            "[[sql.connection]]\nname = \"pg\"\nkind = \"postgres\"\nhost = \"h\"\ndatabase = \"orders\"\nuser = \"u\"\npassword_kv = \"contoso-kv/pg\"\n",
        )
        .unwrap();
        let spec = sql.connection(&ctx(), None).unwrap();
        assert_eq!((spec.kind, spec.port), (Kind::Postgres, 5432));
        assert!(spec.encrypt && !spec.trust_cert && !spec.az);
        assert_eq!(spec.database.as_deref(), Some("orders"));
        let plain = load(
            "[[sql.connection]]\nname = \"pg\"\nkind = \"postgres\"\nhost = \"h\"\nport = 15432\ndatabase = \"d\"\nuser = \"u\"\nencrypt = false\n",
        )
        .unwrap();
        let spec = plain.connection(&ctx(), None).unwrap();
        assert!(!spec.encrypt && spec.port == 15432);
    }

    #[test]
    fn auth_az_on_postgres_signs_in_as_the_named_principal_with_the_oss_rdbms_token() {
        let sql = load(&format!("{FLEXIBLE}user = \"contoso-pg-readers\"\n")).unwrap();
        let spec = sql.connection(&ctx(), None).unwrap();
        assert!(spec.az && spec.encrypt);
        assert_eq!(spec.user, "contoso-pg-readers");
        assert_eq!(
            spec.password(&ctx()).unwrap().expose(),
            "token@https://ossrdbms-aad.database.windows.net"
        );
        assert!(
            spec.refreshed(&ctx()).unwrap().is_none(),
            "a token is not rotated"
        );
        for extra in ["", "user = \"u\"\npassword = \"p\""] {
            assert!(
                failure(&format!("{FLEXIBLE}{extra}\n")).contains("takes user (the Entra user"),
                "{extra}"
            );
        }
    }

    #[test]
    fn an_unknown_connection_lists_the_ones_there_are_and_the_only_one_is_the_default() {
        let sql = load(MSSQL).unwrap();
        assert_eq!(sql.connection(&ctx(), None).unwrap().name, "local-mssql");
        let message = format!("{:#}", sql.connection(&ctx(), Some("prod")).unwrap_err());
        assert_eq!(
            message,
            "no connection \"prod\"; --conn takes one of: local-mssql"
        );
        let two = load(&format!("{MSSQL}{}", MSSQL.replace("local-mssql", "other"))).unwrap();
        let message = format!("{:#}", two.connection(&ctx(), None).unwrap_err());
        assert_eq!(
            message,
            "more than one connection is configured; name one with --conn: local-mssql, other"
        );
        let none = load("").unwrap();
        assert!(
            format!("{:#}", none.connection(&ctx(), Some("x")).unwrap_err())
                .starts_with("no [[sql.connection]] in /cfg/config.toml")
        );
    }

    #[test]
    fn a_password_comes_from_the_file_a_variable_or_a_command() {
        use agent_cli_core::testing::{FakeTransport, ctx};
        let ctx = ctx(agent_cli_core::Setup::fake(FakeTransport::default())
            .with_env("SQL_TEST_PASSWORD", "from-env-1"));
        let sql = load(MSSQL).unwrap();
        let spec = sql.connection(&ctx, Some("local-mssql")).unwrap();
        assert_eq!(spec.password(&ctx).unwrap().expose(), "hunter2hunter2");

        let with = |source: (Option<&str>, Option<&str>)| {
            let mut spec = spec.clone();
            spec.password = Credential::from_keys(
                "password",
                None,
                source.0.map(str::to_owned),
                source.1.map(str::to_owned),
                None,
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
