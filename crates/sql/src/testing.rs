//! What the crate's tests share: a config whose servers never answer, and
//! a run of the domain over it.

use agent_cli_core::Setup;
use agent_cli_core::testing::{FakeTransport, Outcome, run};

use crate::DOMAIN;

/// Nothing listens on port 9 (discard), so a test that did connect would
/// fail fast rather than reach a database.
pub(crate) const CONFIG: &str = r#"
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

pub(crate) fn sql(argv: &[&str], setup: Setup) -> Outcome {
    run(&[DOMAIN], argv, setup)
}

pub(crate) fn setup() -> Setup {
    Setup::fake(FakeTransport::default()).with_config(CONFIG)
}
