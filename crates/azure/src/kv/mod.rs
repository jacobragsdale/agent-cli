//! `kv`: Key Vault secrets. Ported from az-tui's `azure::vault` and its
//! `secrets` / `secret get` commands, with the TUI's version history.
//!
//! Only `kv secret get` returns a value. [`SecretRow`] and [`VersionRow`]
//! have no field one could go in, and a test holds the listing JSON to that.
//! The vault token goes to no host but an `https://` one under
//! `.vault.azure.net`, checked on every call: the first page, each
//! `nextLink`, and vault addresses read back from the cache.

pub(crate) mod secret;
pub(crate) mod vault;
pub(crate) mod version;

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Failure, Request, host_under, percent_encode};
use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Kind, VAULT, bearer, from_unix, narrow, parallel, text};
use crate::config::Azure;
use crate::graph::{Vault, inventory};

/// Key Vault has moved to date-based versions; 7.4 is still what every vault
/// answers, and none of the shapes read here changed. The one constant to bump
/// if a vault refuses it.
pub(crate) const API_VERSION: &str = "7.4";
/// The service refuses a larger page.
const PAGE: usize = 25;

const VAULTS: Kind = Kind {
    noun: "vault",
    key: "vaults",
    flag: "--vault",
    domain: "kv",
};

// ---------- the data plane ----------

/// One signed read of one vault. Every call in this file goes through here.
pub(crate) fn get(ctx: &Ctx, vault: &Vault, url: &str) -> Result<Value> {
    if !host_under(url, ".vault.azure.net") {
        bail!(
            "{}: {url} is not a Key Vault address; the vault token is not sent there",
            vault.name
        );
    }
    let mint = bearer(ctx, VAULT);
    ctx.read(Request::get(url).auth(&mint))
        .map_err(|error| explain(vault, error))?
        .json()
}

/// The two refusals worth saying in fewer words than the service does.
fn explain(vault: &Vault, error: anyhow::Error) -> anyhow::Error {
    if crate::client::refused_with(&error) != Some(403) {
        return error;
    }
    let said = format!("{error:#}");
    if said.contains("Client address") || said.contains("ForbiddenByFirewall") {
        return error.context(format!(
            "{}: blocked by the vault firewall (this machine's IP is not allowed)",
            vault.name
        ));
    }
    error.context(format!(
        "{}: no permission to read secrets (needs the Key Vault Secrets User role or a get/list access policy)",
        vault.name
    ))
}

/// The vault's base, with exactly one trailing slash however it was written.
pub(crate) fn base(vault: &Vault) -> String {
    format!("{}/", vault.uri.trim_end_matches('/'))
}

fn segment(raw: &str) -> String {
    let mut encoded = String::new();
    percent_encode(raw, &mut encoded);
    encoded
}

/// Every entry of a paged listing, following `nextLink` as written.
fn pages(ctx: &Ctx, vault: &Vault, first: String) -> Result<Vec<Value>> {
    let mut entries = Vec::new();
    let mut url = Some(first);
    while let Some(next) = url {
        let page = get(ctx, vault, &next)?;
        entries.extend(page["value"].as_array().into_iter().flatten().cloned());
        url = text(&page["nextLink"]);
    }
    Ok(entries)
}

/// One secret, as a listing describes it. There is no field for its value.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct SecretRow {
    /// `vault/name`: what `kv secret get` and `kv version list` take.
    id: String,
    vault: String,
    name: String,
    enabled: bool,
    content_type: Option<String>,
    /// RFC 3339.
    expires: Option<String>,
    created: Option<String>,
    updated: Option<String>,
    /// A certificate's backing secret, which the vault manages itself.
    managed: bool,
    tags: BTreeMap<String, String>,
}

fn secrets(ctx: &Ctx, vault: &Vault) -> Result<Vec<SecretRow>> {
    let first = format!(
        "{}secrets?api-version={API_VERSION}&maxresults={PAGE}",
        base(vault)
    );
    let mut rows: Vec<SecretRow> = pages(ctx, vault, first)?
        .iter()
        .filter_map(|entry| {
            let attributes = &entry["attributes"];
            let name = last_segment(entry)?;
            Some(SecretRow {
                id: format!("{}/{name}", vault.name),
                vault: vault.name.clone(),
                name,
                // A vault says so only when a secret is disabled.
                enabled: attributes["enabled"].as_bool().unwrap_or(true),
                content_type: text(&entry["contentType"]),
                expires: from_unix(&attributes["exp"]),
                created: from_unix(&attributes["created"]),
                updated: from_unix(&attributes["updated"]),
                managed: entry["managed"].as_bool().unwrap_or(false),
                tags: entry["tags"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_owned())
                    })
                    .collect(),
            })
        })
        .collect();
    rows.sort_by_key(|row| row.name.to_ascii_lowercase());
    Ok(rows)
}

fn last_segment(entry: &Value) -> Option<String> {
    let id = text(&entry["id"])?;
    id.rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// A secret as an agent was handed it.
struct SecretRef {
    vault: Option<String>,
    name: String,
    version: Option<String>,
}

/// `db-password`, the id a listing prints (`kv-contoso-prod/db-password`, and
/// `/version` after it), or the secret's URI
/// (`https://kv-contoso-prod.vault.azure.net/secrets/db-password[/version]`,
/// read, never fetched). A vault or version in it that disagrees with
/// `--vault` or `--version` is exit 2: the CLI never picks one.
fn secret_ref(raw: &str, vault: Option<&str>, version: Option<&str>) -> Result<SecretRef> {
    let raw = raw.trim();
    let bad = |why: &str| -> anyhow::Error {
        Failure::usage(format!("{raw:?} {why}"))
            .hint("pass NAME, VAULT/NAME, VAULT/NAME/VERSION or the secret's URI")
            .into()
    };
    let parts: Vec<String> = match raw.strip_prefix("https://") {
        Some(rest) => {
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            let Some(held) = host
                .to_ascii_lowercase()
                .strip_suffix(".vault.azure.net")
                .map(str::to_owned)
            else {
                return Err(bad("is not a Key Vault secret URI"));
            };
            let path = path.split('?').next().unwrap_or_default();
            let Some(rest) = path.strip_prefix("secrets/") else {
                return Err(bad("is not a Key Vault secret URI"));
            };
            std::iter::once(held)
                .chain(
                    rest.split('/')
                        .filter(|part| !part.is_empty())
                        .map(str::to_owned),
                )
                .collect()
        }
        None => raw.split('/').map(str::to_owned).collect(),
    };
    let (held_vault, name, held_version) = match parts.as_slice() {
        [name] => (None, name.as_str(), None),
        [vault, name] => (Some(vault.as_str()), name.as_str(), None),
        [vault, name, version] => (Some(vault.as_str()), name.as_str(), Some(version.as_str())),
        _ => return Err(bad("is not a secret name, id or URI")),
    };
    if name.is_empty() || held_vault.is_some_and(str::is_empty) {
        return Err(bad("is not a secret name, id or URI"));
    }
    let agree = |held: Option<&str>, flag: Option<&str>, what: &str| -> Result<Option<String>> {
        match (held, flag) {
            (Some(held), Some(flag)) if !held.eq_ignore_ascii_case(flag) => Err(Failure::usage(
                format!("{raw} names {what} {held}, and --{what} says {flag}"),
            )
            .into()),
            (held, flag) => Ok(held.or(flag).map(str::to_owned)),
        }
    };
    Ok(SecretRef {
        vault: agree(held_vault, vault, "vault")?,
        name: name.to_owned(),
        version: agree(held_version, version, "version")?,
    })
}

/// The one vault holding `name`. With `--vault`, or a single vault in reach,
/// that vault; otherwise every vault is listed and exactly one must hold it:
/// two is exit 2 naming both, and guessing would be the worst answer.
fn holder(ctx: &Ctx, name: &str, only: Option<&str>) -> Result<Vault> {
    let azure = Azure::load(ctx)?;
    let only: Vec<String> = only.map(str::to_owned).into_iter().collect();
    let vaults = narrow(
        &inventory(ctx, &azure)?.vaults,
        &azure.vaults,
        &only,
        VAULTS,
        |v| &v.name,
    )?;
    match vaults.as_slice() {
        [] => {
            return Err(Failure::setup("the login reaches no key vaults")
                .hint("az login, or check the subscriptions with `agent-cli doctor kv`")
                .into());
        }
        [one] => return Ok(one.clone()),
        _ => {}
    }
    let mut holding = Vec::new();
    let mut failed = Vec::new();
    for (vault, listing) in vaults
        .iter()
        .zip(parallel(&vaults, azure.parallel(), |vault| {
            secrets(ctx, vault)
        }))
    {
        match listing {
            Ok(rows) if rows.iter().any(|row| row.name.eq_ignore_ascii_case(name)) => {
                holding.push(vault.clone());
            }
            Ok(_) => {}
            Err(error) => failed.push(error),
        }
    }
    // A vault that did not answer can be ruled neither in nor out.
    if holding.is_empty()
        && let Some(error) = failed.into_iter().next()
    {
        return Err(error);
    }
    let names = |vaults: &[Vault]| {
        vaults
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match holding.len() {
        1 => Ok(holding.remove(0)),
        0 => Err(
            Failure::not_found(format!("no secret {name} in {}", names(&vaults)))
                .hint(format!(
                    "agent-cli kv secret list {name} --fields vault,name"
                ))
                .into(),
        ),
        _ => Err(Failure::usage(format!(
            "{name} is in more than one vault ({}); name one with --vault",
            names(&holding)
        ))
        .hint(format!(
            "pass its id: agent-cli kv secret list {name} --fields id,vault"
        ))
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::kv::secret::list::SECRET_LIST;
    use crate::kv::vault::list::VAULT_LIST;
    use crate::kv::version::list::VERSION_LIST;
    use crate::testing::{self, azure, listing, two_vaults};

    #[test]
    fn the_json_of_a_listing_can_never_carry_a_value() {
        let (outcome, _) = azure(
            &[KV],
            &["kv", "secret", "list", "--raw"],
            vec![
                two_vaults(),
                listing("kv-contoso-dev", &["api-key"]),
                listing("kv-contoso-prod", &[]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], "kv-contoso-dev/api-key");
        assert_eq!(rows[0]["name"], "api-key");
        assert_eq!(rows[0]["tags"]["owner"], "platform");
        assert_eq!(rows[0]["created"], "2024-03-01T00:00:00Z");
        assert!(!outcome.stdout.contains("\"value\""), "{}", outcome.stdout);
        for command in [SECRET_LIST, VERSION_LIST, VAULT_LIST] {
            let schema = serde_json::to_string(&(command.returns)()).unwrap();
            assert!(
                !schema.contains("\"value\""),
                "{}: {schema}",
                command.path.join(" ")
            );
        }
    }

    #[test]
    fn a_listing_follows_next_link_verbatim_with_the_vault_token() {
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "secret", "list"],
            vec![
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                Answer::json(&json!({
                    "value": [{"id": "https://kv-contoso-dev.vault.azure.net/secrets/b"}],
                    "nextLink": "https://kv-contoso-dev.vault.azure.net/secrets?api-version=7.4&$skiptoken=XYZ",
                })),
                listing("kv-contoso-dev", &["a"]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            (rows[0]["name"].as_str(), rows[1]["name"].as_str()),
            (Some("a"), Some("b")),
            "both pages, by name"
        );
        let sent = transport.sent();
        assert_eq!(
            sent[1].url,
            "https://kv-contoso-dev.vault.azure.net/secrets?api-version=7.4&maxresults=25"
        );
        assert_eq!(
            sent[2].url,
            "https://kv-contoso-dev.vault.azure.net/secrets?api-version=7.4&$skiptoken=XYZ"
        );
        assert_eq!(
            sent[1].authorization.as_deref(),
            Some("Bearer token@https://vault.azure.net")
        );
    }

    #[test]
    fn a_next_link_off_the_vault_is_refused_without_sending_the_token() {
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "secret", "list"],
            vec![
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                Answer::json(&json!({
                    "value": [{"id": "https://kv-contoso-dev.vault.azure.net/secrets/a"}],
                    "nextLink": "https://evil.example/secrets?api-version=7.4&$skiptoken=XYZ",
                })),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("https://evil.example/secrets"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("the vault token is not sent there"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 2, "the next page was not asked for");
        assert!(
            transport
                .sent()
                .iter()
                .all(|sent| !sent.url.contains("evil"))
        );
    }

    #[test]
    fn a_vault_uri_off_the_vault_host_gets_no_token_either() {
        let mut bad = testing::vault("kv-contoso-dev");
        bad["vaultUri"] = json!("https://kv-contoso-dev.vault.azure.net.evil.example/");
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "secret", "list"],
            vec![testing::inventory(vec![bad])],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("not a Key Vault address"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1, "only the inventory went out");
    }

    #[test]
    fn a_secret_is_named_by_its_id_or_its_uri_and_a_disagreeing_flag_is_refused() {
        let value = || {
            Answer::json(&json!({"value": "fixture-value-1",
                "id": "https://kv-contoso-prod.vault.azure.net/secrets/db-password/8f3a"}))
        };
        for (argv, url) in [
            (
                &[
                    "kv",
                    "secret",
                    "get",
                    "kv-contoso-prod/db-password",
                    "--reveal",
                ][..],
                "https://kv-contoso-prod.vault.azure.net/secrets/db-password?api-version=7.4",
            ),
            (
                &[
                    "kv",
                    "secret",
                    "get",
                    "kv-contoso-prod/db-password/8f3a",
                    "--reveal",
                ][..],
                "https://kv-contoso-prod.vault.azure.net/secrets/db-password/8f3a?api-version=7.4",
            ),
            (
                &[
                    "kv",
                    "secret",
                    "get",
                    "https://kv-contoso-prod.vault.azure.net/secrets/db-password/8f3a",
                    "--reveal",
                ][..],
                "https://kv-contoso-prod.vault.azure.net/secrets/db-password/8f3a?api-version=7.4",
            ),
        ] {
            let (outcome, transport) = azure(&[KV], argv, vec![two_vaults(), value()]);
            assert_eq!(outcome.code, 0, "{argv:?}: {outcome:?}");
            assert_eq!(
                transport.sent()[1].url,
                url,
                "{argv:?}: no vault was searched"
            );
        }
        let (outcome, transport) = azure(
            &[KV],
            &[
                "kv",
                "secret",
                "get",
                "kv-contoso-prod/db-password",
                "--vault",
                "kv-contoso-dev",
                "--reveal",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("names vault kv-contoso-prod, and --vault says kv-contoso-dev"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().is_empty());

        let (outcome, _) = azure(
            &[KV],
            &[
                "kv",
                "secret",
                "list",
                "kv-contoso-prod/db-password",
                "--fields",
                "id",
            ],
            vec![
                two_vaults(),
                listing("kv-contoso-prod", &["db-password", "db-password-old"]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "kv-contoso-prod/db-password"}]),
            "an id is exact, and reads one vault"
        );
    }

    #[test]
    fn a_firewall_and_a_missing_role_are_told_apart() {
        for (body, want) in [
            (
                r#"{"error":{"code":"Forbidden","message":"Client address is not authorized and caller is not a trusted service.","innererror":{"code":"ForbiddenByFirewall"}}}"#,
                "blocked by the vault firewall",
            ),
            (
                r#"{"error":{"code":"Forbidden","message":"Caller is not authorized to perform action on resource."}}"#,
                "Key Vault Secrets User",
            ),
        ] {
            let (outcome, _) = azure(
                &[KV],
                &["kv", "secret", "list"],
                vec![
                    testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                    Answer::status(403, body),
                ],
            );
            assert_eq!(outcome.code, 1, "{outcome:?}");
            assert!(outcome.stderr.contains(want), "{}", outcome.stderr);
        }
    }
}
