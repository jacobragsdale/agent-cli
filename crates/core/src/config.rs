//! One config file, read once, with each domain taking only its own section.
//!
//! In the source TUIs any config error killed every command. Here the file is
//! parsed to a TOML table up front, and a domain deserializes its `[section]`
//! only when a command asks for it, so a broken `[sql]` cannot stop `ado`.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde_json::json;
use toml::{Table, Value};

use crate::ctx::Ctx;
use crate::error::{Exit, Failure};
use crate::output;
use crate::secret::{redact, sensitive_key};

/// `config.example.toml`, built in: an installed binary has no checkout to
/// point at, so `agent-cli config example` prints it.
const EXAMPLE: &str = include_str!("../../../config.example.toml");

/// Domains whose keys live in another domain's section of the example.
const EXAMPLE_ALIASES: &[(&str, &str)] = &[
    ("kv", "azure"),
    ("acr", "azure"),
    ("aks", "azure"),
    ("dd", "datadog"),
];

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
        let named = std::env::var_os("AGENT_CLI_CONFIG").is_some_and(|path| !path.is_empty());
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            // No default file is an empty config; a file AGENT_CLI_CONFIG
            // names and that is not there is a typo.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !named => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(format!(
                "{} does not exist, and AGENT_CLI_CONFIG names it",
                path.display()
            )),
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
        let fix =
            format!("fix [{name}] in {path}; `agent-cli config example {name}` shows every key");
        // A file that cannot be read or parsed stops every section, not one.
        let table = self.table.as_ref().map_err(|error| {
            Failure::setup(error.clone()).hint(format!(
                "fix the file {path} (no section can be read until it parses); `agent-cli config example` shows every key"
            ))
        })?;
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

impl Config {
    /// What `agent-cli config` prints: the file, each section as the domains
    /// read it (variables applied, credentials masked), and every
    /// `AGENT_CLI_*` variable by name with the key it sets. Never the value a
    /// variable holds, which may be a secret.
    pub(crate) fn describe(&self) -> serde_json::Value {
        let mut names: Vec<String> = match &self.table {
            Ok(table) => table.keys().cloned().collect(),
            Err(_) => Vec::new(),
        };
        let known: Vec<String> = example_sections()
            .into_iter()
            .chain(["dd"])
            .map(str::to_owned)
            .chain(names.clone())
            .collect();
        let mut variables = Vec::new();
        for (variable, _) in &self.env {
            let section = known
                .iter()
                .find(|section| variable.starts_with(&env_prefix(section)));
            let Some(section) = section else {
                variables.push(json!({"variable": variable}));
                continue;
            };
            let key = variable[env_prefix(section).len()..].to_ascii_lowercase();
            variables.push(json!({"variable": variable, "section": section, "key": key}));
            if !names.contains(section) {
                names.push(section.clone());
            }
        }
        let sections: serde_json::Map<String, serde_json::Value> = names
            .into_iter()
            .map(|name| {
                let value = match self.section::<Table>(&name) {
                    Ok(table) => mask(serde_json::to_value(table).unwrap_or_default()),
                    Err(error) => json!({"problem": format!("{error:#}")}),
                };
                (name, value)
            })
            .collect();
        json!({
            "path": self.path.display().to_string(),
            "exists": self.path.is_file(),
            "problem": self.problem(),
            "sections": sections,
            "variables": variables,
        })
    }
}

/// A section with every literal credential masked. `*_env` keys name a
/// variable and stay; `*_cmd` keys are masked, as `Credential::source` does,
/// since a command line can carry the secret itself.
fn mask(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value as Json;
    match value {
        Json::String(text) => Json::String(redact(&text)),
        Json::Array(items) => Json::Array(items.into_iter().map(mask).collect()),
        Json::Object(map) => Json::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let secret = !key.ends_with("_env")
                        && (sensitive_key(&key) || key == "pat" || key.ends_with("_cmd"));
                    let value = if secret && !value.is_object() && !value.is_array() {
                        Json::String("***".to_owned())
                    } else {
                        mask(value)
                    };
                    (key, value)
                })
                .collect(),
        ),
        other => other,
    }
}

/// The sections `config.example.toml` has, in its order.
fn example_sections() -> Vec<&'static str> {
    let mut names: Vec<&str> = Vec::new();
    for name in EXAMPLE.lines().filter_map(header) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The section a `# [name]` or `# [[name.item]]` line of the example opens.
fn header(line: &str) -> Option<&str> {
    let name = line.strip_prefix("# [")?.trim_start_matches('[');
    let end = name.find(['.', ']'])?;
    Some(&name[..end])
}

/// `config.example.toml`, or the block for one section or for a domain that
/// reads another's (`kv`). An unknown name is exit 2 naming the known ones.
pub(crate) fn example(name: Option<&str>) -> Result<String, Failure> {
    let Some(name) = name else {
        return Ok(EXAMPLE.trim_end().to_owned());
    };
    let section = EXAMPLE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, section)| section);
    let mut block = Vec::new();
    for line in EXAMPLE.lines() {
        match header(line) {
            Some(opened) if opened == section => block.push(line),
            Some(_) if !block.is_empty() => break,
            _ if !block.is_empty() => block.push(line),
            _ => {}
        }
    }
    if block.is_empty() {
        let known: Vec<&str> = example_sections()
            .into_iter()
            .chain(EXAMPLE_ALIASES.iter().map(|(alias, _)| *alias))
            .collect();
        return Err(
            Failure::usage(format!("no config example for {name:?}")).hint(format!(
                "agent-cli config example takes one of: {}",
                known.join(", ")
            )),
        );
    }
    Ok(block.join("\n").trim_end().to_owned())
}

/// `agent-cli config`: what was read, as JSON. `config example`: the TOML to
/// paste, as text, since TOML is what the config file takes.
pub(crate) fn builtin(
    args: &[String],
    ctx: &Ctx,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tty: bool,
) -> Result<Exit> {
    let usage = "usage: agent-cli config                    the config file, its sections as read (credentials masked) and the AGENT_CLI_* variables set, as JSON\n       agent-cli config example [DOMAIN]  the TOML to paste into the config file: every section, or one domain's";
    if ctx.globals().help {
        writeln!(out, "{usage}")?;
        return Ok(Exit::Ok);
    }
    match args {
        [] => {
            let config = ctx.config();
            output::emit(config.describe(), ctx.globals(), false, out, err, tty)?;
            if config.problem().is_none() && !config.found() {
                writeln!(
                    err,
                    "[no config file; agent-cli config example DOMAIN prints a section to paste into {}, then agent-cli doctor DOMAIN checks it]",
                    config.path().display()
                )?;
            }
        }
        [example] if example == "example" => writeln!(out, "{}", self::example(None)?)?,
        [example, name] if example == "example" => {
            writeln!(out, "{}", self::example(Some(name))?)?;
        }
        _ => {
            return Err(Failure::usage("unknown arguments to agent-cli config")
                .hint(usage)
                .into());
        }
    }
    Ok(Exit::Ok)
}

/// The configured item `wanted` names, or the only one when nothing is
/// named: the one rule for every scope flag (`--cluster`, `--conn`, an
/// instance or site). An unknown name, or a choice left open among several,
/// is exit 2 listing what is configured; nothing configured is exit 3.
pub fn pick<'a, T>(
    noun: &str,
    flag: &str,
    wanted: Option<&str>,
    items: &'a [T],
    name: impl Fn(&T) -> &str,
) -> Result<&'a T, Failure> {
    let names = || items.iter().map(&name).collect::<Vec<_>>().join(", ");
    match (wanted, items) {
        (_, []) => Err(Failure::setup(format!("no {noun} is configured"))
            .hint("`agent-cli config example DOMAIN` prints the block to add; `agent-cli doctor` checks it")),
        (Some(wanted), _) => items
            .iter()
            .find(|item| name(item) == wanted)
            .ok_or_else(|| {
                Failure::usage(format!(
                    "no {noun} {wanted:?}; {flag} takes one of: {}",
                    names()
                ))
            }),
        (None, [only]) => Ok(only),
        (None, _) => Err(Failure::usage(format!(
            "more than one {noun} is configured; name one with {flag}: {}",
            names()
        ))),
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
    fn a_scope_defaults_to_the_only_one_and_otherwise_names_the_choices() {
        let one = ["prod"];
        let two = ["dev", "prod"];
        fn name<'a>(item: &'a &str) -> &'a str {
            item
        }
        assert_eq!(
            pick("scope", "--cluster", None, &one, name).unwrap(),
            &"prod"
        );
        assert_eq!(
            pick("scope", "--cluster", Some("dev"), &two, name).unwrap(),
            &"dev"
        );
        let open = pick("scope", "--cluster", None, &two, name).unwrap_err();
        assert_eq!(
            (open.exit, open.message.as_str()),
            (
                Exit::Usage,
                "more than one scope is configured; name one with --cluster: dev, prod"
            )
        );
        let unknown = pick("scope", "--cluster", Some("qa"), &two, name).unwrap_err();
        assert_eq!(
            unknown.message,
            "no scope \"qa\"; --cluster takes one of: dev, prod"
        );
        let none: [&str; 0] = [];
        assert_eq!(
            pick("scope", "--cluster", None, &none, name)
                .unwrap_err()
                .exit,
            Exit::Setup
        );
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
    fn the_example_prints_whole_or_one_domains_section_and_names_the_rest() {
        assert!(
            example(None)
                .unwrap()
                .starts_with("# agent-cli configuration")
        );
        let sql = example(Some("sql")).unwrap();
        assert!(sql.starts_with("# [sql]") && sql.contains("# [[sql.connection]]"));
        assert!(!sql.contains("airflow.instance"), "{sql}");
        assert_eq!(
            example(Some("kv")).unwrap(),
            example(Some("azure")).unwrap()
        );
        assert!(example(Some("dd")).unwrap().starts_with("# [datadog]"));
        let unknown = example(Some("jira")).unwrap_err();
        assert_eq!(unknown.exit, Exit::Usage);
        assert!(
            unknown
                .hint
                .unwrap()
                .contains("ado, azure, k8s, sql, airflow, datadog, kv"),
            "every section and alias is named"
        );
    }

    #[test]
    fn describe_masks_literal_credentials_and_never_prints_a_variables_value() {
        let config = Config::parse(
            "c.toml",
            Some(
                "[ado]\norg = \"contoso\"\npat = \"pat-literal\"\n\n[[sql.connection]]\n\
                 name = \"r\"\npassword = \"pw-literal\"\npassword_env = \"R_PASSWORD\"\n\
                 password_cmd = \"echo cmd-literal\"\n",
            ),
            env(&[
                ("AGENT_CLI_ADO_PROJECT", "web-from-env"),
                ("AGENT_CLI_DD_API_KEY", "key-from-env"),
                ("AGENT_CLI_READ_ONLY", "1"),
            ]),
        );
        let described = config.describe();
        let text = described.to_string();
        for secret in ["pat-literal", "pw-literal", "cmd-literal", "key-from-env"] {
            assert!(!text.contains(secret), "{secret} printed: {text}");
        }
        let sql = &described["sections"]["sql"]["connection"][0];
        assert_eq!(sql["password_env"], "R_PASSWORD");
        assert_eq!(described["sections"]["ado"]["project"], "web-from-env");
        assert_eq!(described["sections"]["dd"]["api_key"], "***");
        assert_eq!(
            described["variables"],
            json!([
                {"variable": "AGENT_CLI_ADO_PROJECT", "section": "ado", "key": "project"},
                {"variable": "AGENT_CLI_DD_API_KEY", "section": "dd", "key": "api_key"},
                {"variable": "AGENT_CLI_READ_ONLY"},
            ])
        );
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
