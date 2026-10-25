//! `KEY_kv = "VAULT/SECRET"`: a shared login kept in Key Vault, read with
//! the person's own `az` login and kept in `secrets.toml` beside the config.
//!
//! A team rotates one password in one vault instead of asking everyone to
//! edit their file. Reading Key Vault costs an `az` token and a request, so
//! the value is kept (0600, written only here) and read again only when it
//! stops signing in ([`crate::Credential::refresh`]) or on `config pull`.

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde_json::json;
use toml::{Table, Value};

use crate::config::Config;
use crate::ctx::{Ctx, locked};
use crate::error::{Exit, Failure};
use crate::http::{Request, host_under};
use crate::layers::Problem;
use crate::output;
use crate::secret::Secret;

/// The audience of a Key Vault token.
const VAULT: &str = "https://vault.azure.net";
/// What `kv` reads secrets with.
const API_VERSION: &str = "7.4";

/// `VAULT/SECRET` checked: Key Vault's own rules for both names, so a typo
/// is a config error rather than a request to a stranger's host.
pub(crate) fn parse(key: &str, reference: &str) -> Result<(String, String), String> {
    let wrong = || {
        format!(
            "{key}_kv {reference:?} is not VAULT/SECRET (a vault's name, not its URL, then the secret's)"
        )
    };
    let (vault, secret) = reference.trim().split_once('/').ok_or_else(wrong)?;
    let fits = |name: &str, max: usize| {
        !name.is_empty()
            && name.len() <= max
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    };
    if !fits(vault, 24) || !fits(secret, 127) {
        return Err(wrong());
    }
    Ok((vault.to_ascii_lowercase(), secret.to_owned()))
}

/// What every secret `KEY_kv = true` names starts with, so a team's are
/// found, and cleared, together in a vault that holds others.
const PREFIX: &str = "agentcli";

/// `KEY_kv = true` as `"VAULT/SECRET"`: the secret `[core] vault` holds for
/// the key's place in the config, so a team writes each entry without its
/// secret and the vault's name for it follows from the entry's.
/// `[[sql.connection]] name = "reporting-prod"` with `password_kv = true`
/// reads `agentcli-sql-reporting-prod-password`: the section, each table's
/// key or list entry's `name`, then the key, lower case, with anything but
/// letters and digits as `-`.
pub(crate) fn name_secrets(table: &mut Table) -> Result<(), Problem> {
    fn walk(value: &mut Value, at: &mut Vec<String>, vault: Option<&str>) -> Result<(), Problem> {
        match value {
            Value::Table(table) => {
                for (key, value) in table.iter_mut() {
                    match (key.strip_suffix("_kv"), value.as_bool()) {
                        (Some(stem), Some(true)) => {
                            let place = format!("{}.{key}", at.join("."));
                            let Some(vault) = vault else {
                                return Err(Problem {
                                    message: format!(
                                        "{place} = true names its secret in [core] vault, which is not set"
                                    ),
                                    hint: Some(
                                        "add vault = \"VAULT\" (the Key Vault's name) under [core] in the team's file; `agent-cli config` lists the files read"
                                            .to_owned(),
                                    ),
                                });
                            };
                            let mut name = PREFIX.to_owned();
                            for part in at.iter().map(String::as_str).chain([stem]) {
                                for word in part
                                    .split(|c: char| !c.is_ascii_alphanumeric())
                                    .filter(|word| !word.is_empty())
                                {
                                    name.push('-');
                                    name.push_str(&word.to_ascii_lowercase());
                                }
                            }
                            *value = Value::String(format!("{vault}/{name}"));
                        }
                        (Some(_), Some(false)) => {
                            return Err(Problem::plain(format!(
                                "{}.{key} is true (the secret [core] vault holds for it) or \"VAULT/SECRET\", not false",
                                at.join(".")
                            )));
                        }
                        _ => {
                            // A list's entries are named by their own `name`.
                            let list = value.is_array();
                            if !list {
                                at.push(key.clone());
                            }
                            walk(value, at, vault)?;
                            if !list {
                                at.pop();
                            }
                        }
                    }
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter_mut().enumerate() {
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .map_or_else(|| index.to_string(), str::to_owned);
                    at.push(name);
                    walk(item, at, vault)?;
                    at.pop();
                }
            }
            _ => {}
        }
        Ok(())
    }
    let vault = table
        .get("core")
        .and_then(|core| core.get("vault"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|vault| !vault.is_empty())
        .map(str::to_owned);
    for (section, value) in table.iter_mut() {
        if section != "core" {
            walk(value, &mut vec![section.clone()], vault.as_deref())?;
        }
    }
    Ok(())
}

/// The secret's current value, from Key Vault.
pub(crate) fn read(ctx: &Ctx, key: &str, vault: &str, secret: &str) -> Result<Secret> {
    let url = format!("https://{vault}.vault.azure.net/secrets/{secret}?api-version={API_VERSION}");
    if !host_under(&url, ".vault.azure.net") {
        anyhow::bail!("refusing to send a Key Vault token to {url}");
    }
    let mint = ctx.az_bearer(VAULT);
    let answer = ctx.read(Request::get(&url).auth(&mint)).map_err(|error| {
        let (exit, why) = match error.downcast_ref::<Failure>() {
            Some(failure) => (failure.exit, failure.message.clone()),
            None => (crate::error::Exit::Setup, format!("{error:#}")),
        };
        let mut failure = Failure::setup(format!(
            "{key}_kv {vault}/{secret} could not be read from Key Vault: {why}"
        ))
        .hint(format!(
            "az login, then agent-cli config pull; reading it needs the Key Vault Secrets User role on {vault}"
        ));
        if exit == crate::error::Exit::TimedOut {
            failure.exit = exit;
        }
        anyhow::Error::new(failure)
    })?;
    let value = answer.json()?;
    match value["value"].as_str() {
        Some(text) if !text.is_empty() => Ok(Secret::new(text)),
        _ => Err(Failure::setup(format!("{key}_kv {vault}/{secret} is empty in Key Vault")).into()),
    }
}

/// The value kept for `reference`: this run's, else `secrets.toml`'s.
pub(crate) fn kept(ctx: &Ctx, reference: &str) -> Option<Secret> {
    if let Some(held) = locked(&ctx.kept).get(reference) {
        return Some(held.clone());
    }
    let stored = load(ctx.secrets.as_deref()?)
        .get(reference)?
        .as_str()
        .map(Secret::new)?;
    locked(&ctx.kept).insert(reference.to_owned(), stored.clone());
    Some(stored)
}

/// Keeps `value` for `reference`, in this run and in `secrets.toml`.
pub(crate) fn keep(ctx: &Ctx, reference: &str, value: &Secret) -> Result<()> {
    locked(&ctx.kept).insert(reference.to_owned(), value.clone());
    let Some(path) = ctx.secrets.as_deref() else {
        return Ok(());
    };
    let mut table = load(path);
    table.insert(
        reference.to_owned(),
        Value::String(value.expose().to_owned()),
    );
    store(path, &table)
}

/// `secrets.toml` as a table; a missing or broken file is empty, since
/// every value in it can be read again.
pub(crate) fn load(path: &Path) -> Table {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or_default()
}

/// Replaces `secrets.toml` whole: a 0600 temporary file renamed over it.
pub(crate) fn store(path: &Path, table: &Table) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    let text = format!(
        "# Written by agent-cli (config pull, and when a *_kv login is first used). Do not edit.\n{}",
        toml::to_string(table)?
    );
    file.write_all(text.as_bytes())?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// `agent-cli config pull`: every `*_kv` key read from Key Vault again and
/// `secrets.toml` rewritten with them; a key that fails keeps its old
/// value, and one no longer in the config is dropped. Exit 1 when any fails.
pub(crate) fn pull(ctx: &Ctx, out: &mut dyn Write, err: &mut dyn Write, tty: bool) -> Result<Exit> {
    let keys = kv_keys(ctx.config());
    let old = ctx.secrets.as_deref().map(load).unwrap_or_default();
    let mut kept = Table::new();
    let mut rows = Vec::new();
    for (section, at, key, reference) in keys {
        let place = if at.is_empty() {
            key.clone()
        } else {
            format!("{at}.{key}")
        };
        let base = key.strip_suffix("_kv").unwrap_or(&key);
        let mut row = json!({"section": section, "key": place, "kv": reference});
        let read = parse(base, &reference)
            .map_err(|why| anyhow::Error::new(Failure::setup(why)))
            .and_then(|(vault, secret)| {
                let reference = format!("{vault}/{secret}");
                match kept.get(&reference).and_then(Value::as_str) {
                    Some(already) => Ok((reference, already.to_owned())),
                    None => read(ctx, base, &vault, &secret)
                        .map(|value| (reference, value.expose().to_owned())),
                }
            });
        match read {
            Ok((reference, value)) => {
                let changed = old.get(&reference).and_then(Value::as_str) != Some(value.as_str());
                kept.insert(reference, Value::String(value));
                row["ok"] = json!(true);
                row["changed"] = json!(changed);
                row["detail"] = json!(match &ctx.secrets {
                    Some(path) => format!("kept in {}", path.display()),
                    None => "read; nowhere to keep it".to_owned(),
                });
            }
            Err(error) => {
                if let Some(value) = old.get(&reference) {
                    kept.insert(reference.clone(), value.clone());
                }
                let (_, message, hint) = crate::error::describe(&error);
                row["ok"] = json!(false);
                row["changed"] = json!(false);
                row["detail"] = json!(message);
                row["hint"] = json!(hint);
            }
        }
        rows.push(row);
    }
    if let Some(path) = &ctx.secrets
        && (!kept.is_empty() || path.exists())
    {
        store(path, &kept)?;
    }
    let failed = rows.iter().any(|row| row["ok"] == false);
    let none = rows.is_empty();
    output::emit(
        serde_json::Value::Array(rows),
        ctx.globals(),
        false,
        out,
        err,
        tty,
    )?;
    if none {
        writeln!(
            err,
            "[no *_kv key in the config; `agent-cli config example` shows where they go]"
        )?;
    }
    Ok(if failed { Exit::Failed } else { Exit::Ok })
}

/// Every `*_kv` key in the merged file: (section, where in it, the key,
/// `VAULT/SECRET`). Entries of a named list are found by their name.
fn kv_keys(config: &Config) -> Vec<(String, String, String, String)> {
    fn walk(
        value: &Value,
        section: &str,
        at: &str,
        found: &mut Vec<(String, String, String, String)>,
    ) {
        match value {
            Value::Table(table) => {
                for (key, value) in table {
                    let place = if at.is_empty() {
                        key.clone()
                    } else {
                        format!("{at}.{key}")
                    };
                    match (key.strip_suffix("_kv"), value) {
                        (Some(_), Value::String(reference)) => found.push((
                            section.to_owned(),
                            at.to_owned(),
                            key.clone(),
                            reference.clone(),
                        )),
                        _ => walk(value, section, &place, found),
                    }
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .map_or_else(|| index.to_string(), str::to_owned);
                    walk(item, section, &format!("{at}.{name}"), found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    if let Some(table) = config.merged() {
        for (section, value) in table {
            walk(value, section, "", &mut found);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_is_a_vault_name_and_a_secret_name() {
        assert_eq!(
            parse("password", "kv-contoso-team/sql-reader").unwrap(),
            ("kv-contoso-team".to_owned(), "sql-reader".to_owned())
        );
        for wrong in [
            "kv-contoso-team",
            "https://kv.vault.azure.net/secrets/x",
            "kv/a/b",
            "/x",
            "kv.evil.example/x",
        ] {
            assert!(parse("password", wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_kv_key_set_to_true_names_the_secret_for_its_place_in_the_config() {
        let mut table: Table = "[core]\nvault = \"kv-contoso-dev\"\n\n[ado]\npat_kv = true\n\n[datadog]\napi_key_kv = true\n\n[[sql.connection]]\nname = \"Reporting_Prod\"\npassword_kv = true\n\n[[sql.connection]]\nname = \"ledger\"\npassword_kv = \"kv-contoso-prod/ledger-reader\"\n"
            .parse()
            .unwrap();
        name_secrets(&mut table).unwrap();
        assert_eq!(
            table["ado"]["pat_kv"].as_str(),
            Some("kv-contoso-dev/agentcli-ado-pat")
        );
        assert_eq!(
            table["datadog"]["api_key_kv"].as_str(),
            Some("kv-contoso-dev/agentcli-datadog-api-key")
        );
        let connections = table["sql"]["connection"].as_array().unwrap();
        assert_eq!(
            connections[0]["password_kv"].as_str(),
            Some("kv-contoso-dev/agentcli-sql-reporting-prod-password")
        );
        assert_eq!(
            connections[1]["password_kv"].as_str(),
            Some("kv-contoso-prod/ledger-reader"),
            "a named secret stays as written"
        );
        let (vault, secret) =
            parse("password", connections[0]["password_kv"].as_str().unwrap()).unwrap();
        assert_eq!((vault.as_str(), secret.len()), ("kv-contoso-dev", 36));
    }

    #[test]
    fn a_kv_key_set_to_true_without_core_vault_or_set_to_false_is_a_problem_naming_it() {
        let mut table: Table = "[[sql.connection]]\nname = \"prod\"\npassword_kv = true\n"
            .parse()
            .unwrap();
        let problem = name_secrets(&mut table).unwrap_err();
        assert!(
            problem
                .message
                .starts_with("sql.prod.password_kv = true names its secret in [core] vault"),
            "{problem:?}"
        );
        assert!(problem.hint.unwrap().starts_with("add vault = "));
        let mut table: Table = "[core]\nvault = \"kv\"\n\n[ado]\npat_kv = false\n"
            .parse()
            .unwrap();
        assert!(
            name_secrets(&mut table)
                .unwrap_err()
                .message
                .contains("not false")
        );
    }
}
