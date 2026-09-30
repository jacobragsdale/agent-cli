//! One config file, read once, with each domain taking only its own section.
//!
//! In the source TUIs any config error killed every command. Here the file is
//! parsed to a TOML table up front, and a domain deserializes its `[section]`
//! only when a command asks for it, so a broken `[sql]` cannot stop `ado`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::de::DeserializeOwned;
use toml::{Table, Value};

use crate::error::Failure;

pub struct Config {
    path: PathBuf,
    found: bool,
    /// The parsed file, or why it could not be read, for whichever domain asks.
    table: Result<Table, String>,
    /// `AGENT_CLI_*` variables, captured at load so tests need not touch the
    /// process environment.
    env: Vec<(String, String)>,
}

impl Config {
    /// `$AGENT_CLI_CONFIG`, else `$XDG_CONFIG_HOME/agent-cli/config.toml`, else
    /// `~/.config/agent-cli/config.toml` (on macOS too). A missing file is an
    /// empty config.
    #[must_use]
    pub fn load() -> Self {
        let path = default_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("cannot read {}: {error}", path.display())),
        };
        let env = std::env::vars()
            .filter(|(name, _)| name.starts_with("AGENT_CLI_"))
            .collect();
        match text {
            Ok(text) => Self::parse(path, text.as_deref(), env),
            Err(error) => Self {
                path,
                found: true,
                table: Err(error),
                env,
            },
        }
    }

    /// A config from text (`None` for a missing file) and a set of variables.
    #[must_use]
    pub fn parse(path: impl Into<PathBuf>, text: Option<&str>, env: Vec<(String, String)>) -> Self {
        let path = path.into();
        let table = text
            .unwrap_or_default()
            .parse::<Table>()
            .map_err(|error| format!("cannot parse {}: {error}", path.display()));
        Self {
            path,
            found: text.is_some(),
            table,
            env,
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::parse("config.toml", None, Vec::new())
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// False when there is no file and every section is empty.
    #[must_use]
    pub fn found(&self) -> bool {
        self.found
    }

    /// Why the file could not be used, if it could not.
    #[must_use]
    pub fn problem(&self) -> Option<&str> {
        self.table.as_ref().err().map(String::as_str)
    }

    /// True when the file has a `[name]` section or a variable overrides one.
    #[must_use]
    pub fn has_section(&self, name: &str) -> bool {
        let prefix = env_prefix(name);
        self.table
            .as_ref()
            .is_ok_and(|table| table.contains_key(name))
            || self.env.iter().any(|(key, _)| key.starts_with(&prefix))
    }

    /// The `[name]` section as `T`, with every `AGENT_CLI_<NAME>_<KEY>`
    /// variable replacing that scalar key. A missing section deserializes from
    /// an empty table, so a `T` with defaults works without one. Any problem is
    /// exit 3, naming the section and the file.
    pub fn section<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        let path = self.path.display();
        let fix = format!("fix [{name}] in {path}; config.example.toml shows every key");
        let table = self
            .table
            .as_ref()
            .map_err(|error| Failure::setup(error.clone()).hint(fix.clone()))?;
        let mut section = match table.get(name) {
            Some(Value::Table(section)) => section.clone(),
            None => Table::new(),
            Some(_) => {
                return Err(Failure::setup(format!("[{name}] in {path} is not a table"))
                    .hint(fix)
                    .into());
            }
        };
        let prefix = env_prefix(name);
        for (variable, raw) in &self.env {
            let Some(key) = variable.strip_prefix(&prefix) else {
                continue;
            };
            let key = key.to_ascii_lowercase();
            let value = override_value(section.get(&key), raw).ok_or_else(|| {
                Failure::setup(format!(
                    "{variable}={raw} does not fit [{name}] {key} in {path}"
                ))
                .hint(format!(
                    "give {variable} a value of the same type as {key}, or unset it"
                ))
            })?;
            section.insert(key, value);
        }
        Value::Table(section)
            .try_into()
            .map_err(|error: toml::de::Error| {
                Failure::setup(format!("[{name}] in {path}: {}", error.message()))
                    .hint(fix)
                    .into()
            })
    }
}

fn env_prefix(section: &str) -> String {
    format!(
        "AGENT_CLI_{}_",
        section.to_ascii_uppercase().replace('-', "_")
    )
}

/// A variable's text as the type the file already gives that key, or inferred
/// (bool, then integer, then string) for a key the file does not have. Lists
/// and tables are not scalars and cannot be overridden.
// ponytail: an inferred key that looks like a number (an org named "123")
// becomes an integer; add the key to the file to pin its type.
fn override_value(existing: Option<&Value>, raw: &str) -> Option<Value> {
    match existing {
        Some(Value::String(_)) => Some(Value::String(raw.to_owned())),
        Some(Value::Integer(_)) => raw.trim().parse().ok().map(Value::Integer),
        Some(Value::Float(_)) => raw.trim().parse().ok().map(Value::Float),
        Some(Value::Boolean(_)) => parse_bool(raw).map(Value::Boolean),
        Some(Value::Datetime(_) | Value::Array(_) | Value::Table(_)) => None,
        None => Some(
            parse_bool(raw)
                .map(Value::Boolean)
                .or_else(|| raw.trim().parse().ok().map(Value::Integer))
                .unwrap_or_else(|| Value::String(raw.to_owned())),
        ),
    }
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn default_path() -> PathBuf {
    if let Some(path) = std::env::var_os("AGENT_CLI_CONFIG").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    xdg_dir("XDG_CONFIG_HOME", ".config")
        .unwrap_or_default()
        .join("agent-cli")
        .join("config.toml")
}

/// `$VAR` when it is an absolute path (the XDG spec says to ignore a relative
/// one), else `~/<fallback>`.
pub(crate) fn xdg_dir(variable: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::error::{Exit, describe};

    #[derive(Debug, Default, Deserialize, PartialEq)]
    #[serde(default)]
    struct Ado {
        org: String,
        project: String,
        top: i64,
        verbose: bool,
    }

    #[derive(Debug, Deserialize)]
    struct Sql {
        #[allow(dead_code)]
        connection: Vec<Connection>,
    }

    #[derive(Debug, Deserialize)]
    struct Connection {
        #[allow(dead_code)]
        name: String,
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_broken_section_breaks_only_its_own_domain() {
        let config = Config::parse(
            "/cfg/config.toml",
            Some("[ado]\norg = \"contoso\"\n\n[sql]\nconnection = \"oops\"\n"),
            Vec::new(),
        );
        let ado: Ado = config.section("ado").unwrap();
        assert_eq!(ado.org, "contoso");
        let error = config.section::<Sql>("sql").unwrap_err();
        let (exit, message, hint) = describe(&error);
        assert_eq!(exit, Exit::Setup);
        assert!(
            message.starts_with("[sql] in /cfg/config.toml: "),
            "{message}"
        );
        assert!(hint.unwrap().contains("fix [sql] in /cfg/config.toml"));
        assert!(config.has_section("sql") && !config.has_section("k8s"));
    }

    #[test]
    fn a_missing_file_or_section_is_empty_and_a_syntax_error_names_the_file() {
        let empty = Config::parse("/cfg/config.toml", None, Vec::new());
        assert_eq!(empty.section::<Ado>("ado").unwrap(), Ado::default());
        assert!(empty.problem().is_none());

        let broken = Config::parse("/cfg/config.toml", Some("[ado\norg = 1"), Vec::new());
        let (exit, message, _) = describe(&broken.section::<Ado>("ado").unwrap_err());
        assert_eq!(exit, Exit::Setup);
        assert!(
            message.starts_with("cannot parse /cfg/config.toml"),
            "{message}"
        );
        assert!(broken.problem().is_some());
    }

    #[test]
    fn a_variable_overrides_a_scalar_key_in_its_type() {
        let config = Config::parse(
            "c.toml",
            Some("[ado]\norg = \"contoso\"\ntop = 5\n"),
            env(&[
                ("AGENT_CLI_ADO_ORG", "fabrikam"),
                ("AGENT_CLI_ADO_TOP", "9"),
                ("AGENT_CLI_ADO_VERBOSE", "true"),
                ("AGENT_CLI_ADO_PROJECT", "web"),
                ("AGENT_CLI_SQL_ORG", "ignored"),
            ]),
        );
        let ado: Ado = config.section("ado").unwrap();
        assert_eq!(
            ado,
            Ado {
                org: "fabrikam".into(),
                project: "web".into(),
                top: 9,
                verbose: true
            }
        );

        let bad = Config::parse(
            "c.toml",
            Some("[ado]\ntop = 5\n"),
            env(&[("AGENT_CLI_ADO_TOP", "many")]),
        );
        let (exit, message, _) = describe(&bad.section::<Ado>("ado").unwrap_err());
        assert_eq!(exit, Exit::Setup);
        assert_eq!(
            message,
            "AGENT_CLI_ADO_TOP=many does not fit [ado] top in c.toml"
        );
        assert!(Config::parse("c.toml", None, env(&[("AGENT_CLI_K8S_X", "1")])).has_section("k8s"));
    }
}
