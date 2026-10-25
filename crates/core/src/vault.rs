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
}
