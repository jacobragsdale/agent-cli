//! `kv`: Key Vault secrets, keys and certificates on the data plane, and the
//! vault itself through ARM (`vault get`). Ported from az-tui's
//! `azure::vault` and its `secrets` / `secret get` commands, with the TUI's
//! version history.
//!
//! Only `kv secret get` returns a value. [`SecretRow`] and [`VersionRow`]
//! have no field one could go in, and a test holds the listing JSON to that;
//! a key's material and a certificate's bytes are never printed either.
//! `secret update` takes its value from stdin or a file into a `Secret` and
//! sends it as bytes (`put`), so a plan shows its length only. The vault
//! token goes to no host but an `https://` one under `.vault.azure.net`,
//! checked on every call (`signed`): the first page, each `nextLink`, and
//! vault addresses read back from the cache.
//!
//! A secret's, key's or certificate's id is `vault/name[/version]`, or its
//! URI: `item_ref` reads one in its collection, `holder` finds the vault of a
//! bare name, `Wanted` is a listing's NAME, `across` reads every vault in
//! scope, and [`get`], `found` (a 404 is `None`) and `put` are the doors to
//! the data plane. A vault's id is its name, as for `--vault`.

pub(crate) mod certificate;
pub(crate) mod key;
pub(crate) mod secret;
pub(crate) mod vault;
pub(crate) mod version;

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Effect, Failure, Method, Request, Span, host_under, percent_encode};
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

/// One signed read of one vault. Every read in this file goes through here.
pub(crate) fn get(ctx: &Ctx, vault: &Vault, url: &str) -> Result<Value> {
    let mint = bearer(ctx, VAULT);
    ctx.read(Request::get(signed(vault, url)?).auth(&mint))
        .map_err(|error| explain(vault, url, false, error))?
        .json()
}

/// A read of one vault that may well find nothing (a key with no rotation
/// policy, a certificate with no pending operation): a 404 is `None`.
fn found(ctx: &Ctx, vault: &Vault, url: &str) -> Result<Option<Value>> {
    match get(ctx, vault, url) {
        Err(error) if crate::client::refused_with(&error) == Some(404) => Ok(None),
        other => other.map(Some),
    }
}

/// One signed change to one vault: `body` is sent as JSON bytes, so a
/// `--dry-run` plan shows its length and never what it holds.
fn put(ctx: &Ctx, vault: &Vault, url: &str, body: Vec<u8>) -> Result<Value> {
    let mint = bearer(ctx, VAULT);
    let request = Request::new(Method::Put, signed(vault, url)?)
        .header("Content-Type", "application/json")
        .bytes(body)
        .auth(&mint);
    ctx.write(Effect::Write, request)
        .map_err(|error| explain(vault, url, true, error))?
        .json()
}

/// `url`, when it is the vault's; the vault token goes nowhere else.
fn signed<'a>(vault: &Vault, url: &'a str) -> Result<&'a str> {
    if !host_under(url, ".vault.azure.net") {
        bail!(
            "{}: {url} is not a Key Vault address; the vault token is not sent there",
            vault.name
        );
    }
    Ok(url)
}

/// The two refusals worth saying in fewer words than the service does.
fn explain(vault: &Vault, url: &str, writing: bool, error: anyhow::Error) -> anyhow::Error {
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
    // `https://host/secrets/…`: the collection is the path's first segment.
    let collection = url
        .split('/')
        .nth(3)
        .and_then(|segment| segment.split('?').next())
        .unwrap_or("secrets");
    let (doing, role, policy) = match collection {
        _ if writing => ("set secrets", "Key Vault Secrets Officer", "set"),
        "keys" => ("read keys", "Key Vault Crypto User", "get/list"),
        "certificates" => (
            "read certificates",
            "Key Vault Certificate User",
            "get/list",
        ),
        _ => ("read secrets", "Key Vault Secrets User", "get/list"),
    };
    error.context(format!(
        "{}: no permission to {doing} (needs the {role} role or a {policy} access policy)",
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
    /// With `--deleted`: when it was deleted, and when the vault purges it
    /// for good unless it is recovered.
    deleted: Option<String>,
    purge: Option<String>,
}

/// The secrets of one vault, or with `deleted` the ones in its recycle bin.
fn secrets(ctx: &Ctx, vault: &Vault, deleted: bool) -> Result<Vec<SecretRow>> {
    let collection = if deleted { "deletedsecrets" } else { "secrets" };
    let first = format!(
        "{}{collection}?api-version={API_VERSION}&maxresults={PAGE}",
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
                enabled: enabled(attributes),
                content_type: text(&entry["contentType"]),
                expires: from_unix(&attributes["exp"]),
                created: from_unix(&attributes["created"]),
                updated: from_unix(&attributes["updated"]),
                managed: entry["managed"].as_bool().unwrap_or(false),
                tags: tags(entry),
                deleted: from_unix(&entry["deletedDate"]),
                purge: from_unix(&entry["scheduledPurgeDate"]),
            })
        })
        .collect();
    rows.sort_by_key(|row| row.name.to_ascii_lowercase());
    Ok(rows)
}

/// What a diff compares of a secret, from a listing's entry or a GET:
/// its attributes and tags, not its own dates or version.
fn compared(entry: &Value) -> Value {
    let attributes = &entry["attributes"];
    serde_json::json!({
        "enabled": enabled(attributes),
        "content_type": text(&entry["contentType"]),
        "expires": from_unix(&attributes["exp"]),
        "not_before": from_unix(&attributes["nbf"]),
        "managed": entry["managed"].as_bool().unwrap_or(false),
        "tags": tags(entry),
    })
}

/// A secret's GET for a diff: its compared attributes and its value, kept
/// in a `Secret` (compared, never printed); `None` where the vault will
/// not hand it over (a disabled secret).
fn secret_for_diff(
    ctx: &Ctx,
    vault: &Vault,
    name: &str,
) -> Result<(Value, Option<agent_cli_core::Secret>)> {
    let url = format!(
        "{}secrets/{}?api-version={API_VERSION}",
        base(vault),
        segment(name)
    );
    match get(ctx, vault, &url) {
        Ok(answer) => Ok((
            compared(&answer),
            answer["value"].as_str().map(agent_cli_core::Secret::new),
        )),
        Err(error) if crate::client::refused_with(&error) == Some(403) => {
            let said = format!("{error:#}");
            if said.contains("disabled") || said.contains("Disabled") {
                Ok((serde_json::json!({"enabled": false}), None))
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

/// A vault says so only when an item is disabled.
fn enabled(attributes: &Value) -> bool {
    attributes["enabled"].as_bool().unwrap_or(true)
}

fn tags(entry: &Value) -> BTreeMap<String, String> {
    entry["tags"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_owned()))
        .collect()
}

/// The vaults a listing reads: `[azure] vaults`, then `only` within it.
fn vaults(ctx: &Ctx, azure: &Azure, only: &[String]) -> Result<Vec<Vault>> {
    narrow(
        &inventory(ctx, azure)?.vaults,
        &azure.vaults,
        only,
        VAULTS,
        |v| &v.name,
    )
}

/// Each vault's rows, read side by side. A vault that does not answer is
/// named on stderr and the rest still print; when none answers, the first
/// refusal is the answer.
fn across<T: Send>(
    ctx: &Ctx,
    azure: &Azure,
    vaults: &[Vault],
    read: impl Fn(&Vault) -> Result<Vec<T>> + Sync,
) -> Result<Vec<T>> {
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for listing in parallel(vaults, azure.parallel(), read) {
        match listing {
            Ok(found) => rows.extend(found),
            Err(error) => failed.push(error),
        }
    }
    if !failed.is_empty() && failed.len() == vaults.len() {
        return Err(failed.remove(0));
    }
    for error in failed {
        ctx.note(format!("[not read: {error:#}]"));
    }
    Ok(rows)
}

/// What a listing's NAME narrows to: an id (`vault/name`) reads its one vault
/// and matches the name exactly; anything else is part of a name, any case,
/// in the `--vault` vaults.
struct Wanted {
    only: Vec<String>,
    name: Option<String>,
    exact: bool,
}

impl Wanted {
    fn new(raw: Option<&str>, collection: &str, vault: &[String]) -> Result<Self> {
        let Some(raw) = raw else {
            return Ok(Self {
                only: vault.to_vec(),
                name: None,
                exact: false,
            });
        };
        let found = item_ref(raw, collection, None, None)?;
        Ok(match found.vault {
            Some(held) => Self {
                only: vec![held],
                name: Some(found.name.to_ascii_lowercase()),
                exact: true,
            },
            None => Self {
                only: vault.to_vec(),
                name: Some(raw.to_ascii_lowercase()),
                exact: false,
            },
        })
    }

    fn matches(&self, name: &str) -> bool {
        self.name.as_deref().is_none_or(|wanted| {
            let name = name.to_ascii_lowercase();
            if self.exact {
                name == wanted
            } else {
                name.contains(wanted)
            }
        })
    }
}

/// An item's expiry, for `--expired` and `--expires-within`: something
/// already expired is inside every window.
fn expiring(expires: Option<&str>, expired: bool, within: Option<Span>) -> bool {
    let now = agent_cli_core::now();
    let at = expires.and_then(crate::client::parse_stamp);
    (!expired || at.is_some_and(|at| at < now))
        && within.is_none_or(|within| at.is_some_and(|at| at <= now + within.0))
}

/// The last segment of an entry's `id`, or of a key's `kid`, which keys list by.
fn last_segment(entry: &Value) -> Option<String> {
    let id = text(&entry["id"]).or_else(|| text(&entry["kid"]))?;
    id.rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// A secret, key or certificate as an agent was handed it.
struct ItemRef {
    vault: Option<String>,
    name: String,
    version: Option<String>,
}

/// What one of a vault's `collection`s (`secrets`, `keys`, `certificates`)
/// calls an item of it.
fn noun(collection: &str) -> &str {
    collection.strip_suffix('s').unwrap_or(collection)
}

/// `db-password`, the id a listing prints (`kv-contoso-prod/db-password`, and
/// `/version` after it), or the item's URI
/// (`https://kv-contoso-prod.vault.azure.net/secrets/db-password[/version]`,
/// read, never fetched), in `collection`. A vault or version in it that
/// disagrees with `--vault` or `--version` is exit 2: the CLI never picks one.
fn item_ref(
    raw: &str,
    collection: &str,
    vault: Option<&str>,
    version: Option<&str>,
) -> Result<ItemRef> {
    let raw = raw.trim();
    let noun = noun(collection);
    let bad = |why: String| -> anyhow::Error {
        Failure::usage(format!("{raw:?} {why}"))
            .hint(format!(
                "pass NAME, VAULT/NAME, VAULT/NAME/VERSION or the {noun}'s URI"
            ))
            .into()
    };
    let parts: Vec<String> = match raw.strip_prefix("https://") {
        Some(rest) => {
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            let not_uri = || bad(format!("is not a Key Vault {noun} URI"));
            let Some(held) = host
                .to_ascii_lowercase()
                .strip_suffix(".vault.azure.net")
                .map(str::to_owned)
            else {
                return Err(not_uri());
            };
            let path = path.split('?').next().unwrap_or_default();
            let Some(rest) = path.strip_prefix(&format!("{collection}/")) else {
                return Err(not_uri());
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
    let not_ref = || bad(format!("is not a {noun} name, id or URI"));
    let (held_vault, name, held_version) = match parts.as_slice() {
        [name] => (None, name.as_str(), None),
        [vault, name] => (Some(vault.as_str()), name.as_str(), None),
        [vault, name, version] => (Some(vault.as_str()), name.as_str(), Some(version.as_str())),
        _ => return Err(not_ref()),
    };
    if name.is_empty() || held_vault.is_some_and(str::is_empty) {
        return Err(not_ref());
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
    Ok(ItemRef {
        vault: agree(held_vault, vault, "vault")?,
        name: name.to_owned(),
        version: agree(held_version, version, "version")?,
    })
}

/// The names in one of a vault's collections.
fn names(ctx: &Ctx, vault: &Vault, collection: &str) -> Result<Vec<String>> {
    let first = format!(
        "{}{collection}?api-version={API_VERSION}&maxresults={PAGE}",
        base(vault)
    );
    Ok(pages(ctx, vault, first)?
        .iter()
        .filter_map(last_segment)
        .collect())
}

/// The one vault holding `name` in `collection`. With `--vault`, or a single
/// vault in reach, that vault; otherwise every vault is listed and exactly
/// one must hold it: two is exit 2 naming both, and guessing would be the
/// worst answer.
fn holder(ctx: &Ctx, collection: &str, name: &str, only: Option<&str>) -> Result<Vault> {
    let azure = Azure::load(ctx)?;
    let only: Vec<String> = only.map(str::to_owned).into_iter().collect();
    let vaults = vaults(ctx, &azure, &only)?;
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
            names(ctx, vault, collection)
        }))
    {
        match listing {
            Ok(held) if held.iter().any(|held| held.eq_ignore_ascii_case(name)) => {
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
    let listed = |vaults: &[Vault]| {
        vaults
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match holding.len() {
        1 => Ok(holding.remove(0)),
        0 => Err(Failure::not_found(format!(
            "no {} {name} in {}",
            noun(collection),
            listed(&vaults)
        ))
        .hint(format!(
            "agent-cli kv {} list {name} --fields vault,name",
            noun(collection)
        ))
        .into()),
        _ => Err(Failure::usage(format!(
            "{name} is in more than one vault ({}); name one with --vault",
            listed(&holding)
        ))
        .hint(format!(
            "pass its id: agent-cli kv {} list {name} --fields id,vault",
            noun(collection)
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
