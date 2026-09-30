//! `kv`: Key Vault secrets. Ported from az-tui's `azure::vault` and its
//! `secrets` / `secret get` commands, with the TUI's version history.
//!
//! Only `kv secret get` returns a value. [`SecretRow`] and [`VersionRow`]
//! have no field one could go in, and a test holds the listing JSON to that.
//! The vault token goes to no host but an `https://` one under
//! `.vault.azure.net`, checked on every call: the first page, each
//! `nextLink`, and vault addresses read back from the cache.

use std::collections::BTreeMap;

use agent_cli_core::{
    Check, Config, Ctx, Failure, Request, Secret, command, host_under, percent_encode,
};
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;

use crate::graph::{Vault, inventory};
use crate::{Azure, Kind, VAULT, bearer, from_unix, limited, narrow, parallel, text, window};

/// Key Vault has moved to date-based versions; 7.4 is still what every vault
/// answers, and none of the shapes read here changed. The one constant to bump
/// if a vault refuses it.
const API_VERSION: &str = "7.4";
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
fn get(ctx: &Ctx, vault: &Vault, url: &str) -> Result<Value> {
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
    if crate::refused_with(&error) != Some(403) {
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
fn base(vault: &Vault) -> String {
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
            Some(SecretRow {
                vault: vault.name.clone(),
                name: last_segment(entry)?,
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

// ---------- kv vault list ----------

#[derive(clap::Args)]
pub struct VaultListArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn vault_list(ctx: &Ctx, args: VaultListArgs) -> Result<Vec<Vault>> {
    let azure = Azure::load(ctx)?;
    let vaults = narrow(
        &inventory(ctx, &azure)?.vaults,
        &azure.vaults,
        &[],
        VAULTS,
        |v| &v.name,
    )?;
    Ok(limited(ctx, vaults, args.limit))
}

command! {
    pub VAULT_LIST = ["kv", "vault", "list"], Read,
    "List the key vaults the az login reaches (within [azure] vaults)",
    keywords: ["keyvault", "inventory", "uri", "subscription"],
    example: "kv vault list --fields name,resource_group,uri",
    run: vault_list,
}

// ---------- kv secret list ----------

#[derive(clap::Args)]
pub struct SecretListArgs {
    /// Part of the secret name, any case
    name: Option<String>,
    /// Only this vault (repeatable; within [azure] vaults)
    #[arg(long)]
    vault: Vec<String>,
    /// Only secrets expiring within this window, or already expired: 30d, 12h
    #[arg(long, value_name = "WINDOW")]
    expires_within: Option<String>,
    /// Only secrets whose expiry has passed
    #[arg(long)]
    expired: bool,
    /// Only disabled secrets
    #[arg(long)]
    disabled: bool,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn secret_list(ctx: &Ctx, args: SecretListArgs) -> Result<Vec<SecretRow>> {
    let within = args
        .expires_within
        .as_deref()
        .map(|raw| window("--expires-within", raw))
        .transpose()?;
    let azure = Azure::load(ctx)?;
    let vaults = narrow(
        &inventory(ctx, &azure)?.vaults,
        &azure.vaults,
        &args.vault,
        VAULTS,
        |v| &v.name,
    )?;
    let rows = read_all(ctx, &azure, &vaults)?;
    let now = OffsetDateTime::now_utc();
    let expiry = |row: &SecretRow| row.expires.as_deref().and_then(crate::parse_stamp);
    let wanted = args.name.as_deref().map(str::to_ascii_lowercase);
    let rows = rows
        .into_iter()
        .filter(|row| {
            wanted
                .as_deref()
                .is_none_or(|wanted| row.name.to_ascii_lowercase().contains(wanted))
        })
        .filter(|row| !args.disabled || !row.enabled)
        .filter(|row| !args.expired || expiry(row).is_some_and(|at| at < now))
        .filter(|row| {
            // Something already expired is inside every window.
            within.is_none_or(|within| expiry(row).is_some_and(|at| at <= now + within))
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

/// Every secret in `vaults`, read side by side. A vault that does not answer
/// is named on stderr and the rest still print; when none answers, the first
/// refusal is the answer.
fn read_all(ctx: &Ctx, azure: &Azure, vaults: &[Vault]) -> Result<Vec<SecretRow>> {
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for listing in parallel(vaults, azure.parallel(), |vault| secrets(ctx, vault)) {
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

command! {
    pub SECRET_LIST = ["kv", "secret", "list"], Read,
    "List Key Vault secrets with expiry and tags (names and metadata, never values)",
    keywords: ["password", "credential", "expiring", "expired", "disabled", "vaults", "contains", "find"],
    example: "kv secret list db --expires-within 30d --fields vault,name,expires",
    run: secret_list,
}

// ---------- kv secret get ----------

#[derive(clap::Args)]
pub struct SecretGetArgs {
    /// The secret's exact name
    name: String,
    /// The vault that holds it; needed when more than one does
    #[arg(long)]
    vault: Option<String>,
    /// A version id from `kv version list`; the current one when left out
    #[arg(long)]
    version: Option<String>,
}

/// A secret's value: the one row type with a field for one.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SecretValue {
    vault: String,
    name: String,
    version: String,
    content_type: Option<String>,
    value: String,
}

fn secret_get(ctx: &Ctx, args: SecretGetArgs) -> Result<SecretValue> {
    let vault = holder(ctx, &args.name, args.vault.as_deref())?;
    let url = match &args.version {
        Some(version) => format!(
            "{}secrets/{}/{}?api-version={API_VERSION}",
            base(&vault),
            segment(&args.name),
            segment(version)
        ),
        None => format!(
            "{}secrets/{}?api-version={API_VERSION}",
            base(&vault),
            segment(&args.name)
        ),
    };
    let answer = get(ctx, &vault, &url)?;
    let secret =
        Secret::new(answer["value"].as_str().with_context(|| {
            format!("{} answered without a value for {}", vault.name, args.name)
        })?);
    Ok(SecretValue {
        name: args.name,
        // The id's last segment is the version the vault handed over, which is
        // what "current" means when none was asked for.
        version: last_segment(&answer).unwrap_or_default(),
        content_type: text(&answer["contentType"]),
        vault: vault.name,
        // The one place a vault's value leaves its Secret.
        value: secret.expose().to_owned(),
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
            "agent-cli kv secret get {name} --vault {} ...",
            holding[0].name
        ))
        .into()),
    }
}

command! {
    pub SECRET_GET = ["kv", "secret", "get"], Reveal,
    "Get a Key Vault secret's value (needs --reveal, or --output FILE)",
    keywords: ["password", "credential", "connection", "string", "value", "read", "fetch"],
    example: "kv secret get db-password --vault kv-contoso-dev --fields value --output db-password.txt",
    run: secret_get,
}

// ---------- kv version list ----------

#[derive(clap::Args)]
pub struct VersionListArgs {
    /// The secret's exact name
    secret: String,
    /// The vault that holds it; needed when more than one does
    #[arg(long)]
    vault: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct VersionRow {
    version: String,
    enabled: bool,
    /// RFC 3339.
    created: Option<String>,
    updated: Option<String>,
    expires: Option<String>,
}

fn version_list(ctx: &Ctx, args: VersionListArgs) -> Result<Vec<VersionRow>> {
    let vault = holder(ctx, &args.secret, args.vault.as_deref())?;
    let first = format!(
        "{}secrets/{}/versions?api-version={API_VERSION}&maxresults={PAGE}",
        base(&vault),
        segment(&args.secret)
    );
    let mut rows: Vec<VersionRow> = pages(ctx, &vault, first)?
        .iter()
        .filter_map(|entry| {
            let attributes = &entry["attributes"];
            Some(VersionRow {
                version: last_segment(entry)?,
                enabled: attributes["enabled"].as_bool().unwrap_or(true),
                created: from_unix(&attributes["created"]),
                updated: from_unix(&attributes["updated"]),
                expires: from_unix(&attributes["exp"]),
            })
        })
        .collect();
    // The service promises no order, and the newest is the one wanted. RFC
    // 3339 in UTC sorts as text.
    rows.sort_by(|a, b| b.created.cmp(&a.created));
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub VERSION_LIST = ["kv", "version", "list"], Read,
    "List a secret's versions, newest first (when it was rotated; never values)",
    keywords: ["history", "rotated", "rotation", "previous", "old"],
    example: "kv version list db-password --fields version,created,enabled",
    run: version_list,
}

// ---------- overview and doctor ----------

pub fn status(config: &Config) -> String {
    crate::allowlist_status(config, "kv", "vaults", |azure| &azure.vaults)
}

/// The login and its two tokens, the inventory, and one page from each vault:
/// a vault Resource Graph lists can still refuse every data-plane call. Reads
/// names only, never a value.
pub fn doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("azure") {
        return Vec::new();
    }
    let mut checks = Vec::new();
    let Some(azure) = crate::doctor_login(ctx, &mut checks, &[("vault token", VAULT)]) else {
        return checks;
    };
    let inventory = match inventory(ctx, &azure) {
        Ok(inventory) => inventory,
        Err(error) => {
            checks.push(Check::failed(
                "inventory",
                format!("{error:#}"),
                "az account list; check [azure] subscriptions",
            ));
            return checks;
        }
    };
    let vaults = crate::allowed(&inventory.vaults, &azure.vaults, |v| &v.name);
    checks.push(Check::ok(
        "inventory",
        format!("{} vaults in reach", vaults.len()),
    ));
    for name in crate::missing(&inventory.vaults, &azure.vaults, |v| &v.name) {
        checks.push(Check::failed(
            format!("vault {name}"),
            "not found by Resource Graph in the subscriptions in scope",
            "fix [azure] vaults or subscriptions; `agent-cli kv vault list` shows what is there",
        ));
    }
    for vault in &vaults {
        let check = format!("vault {}", vault.name);
        if ctx.remaining().is_err() {
            checks.push(Check::failed(
                check,
                "not checked: --timeout ran out",
                "agent-cli doctor kv --timeout 120",
            ));
            continue;
        }
        let started = std::time::Instant::now();
        let url = format!(
            "{}secrets?api-version={API_VERSION}&maxresults=1",
            base(vault)
        );
        checks.push(match get(ctx, vault, &url) {
            Ok(_) => Check::ok(
                check,
                format!("answered in {} ms", started.elapsed().as_millis()),
            ),
            Err(error) => Check::failed(
                check,
                format!("{error:#}"),
                "needs the Key Vault Secrets User role, and this IP allowed by the vault firewall",
            ),
        });
    }
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::KV;
    use crate::fixtures::{self, azure, azure_with};

    fn listing(vault: &str, names: &[&str]) -> Answer {
        Answer::json(&json!({
            "value": names.iter().map(|name| json!({
                "id": format!("https://{vault}.vault.azure.net/secrets/{name}"),
                "contentType": "text/plain",
                "tags": {"owner": "platform"},
                "attributes": {"enabled": true, "created": 1_709_251_200, "updated": 1_725_753_600},
            })).collect::<Vec<_>>(),
            "nextLink": null,
        }))
    }

    fn two_vaults() -> Answer {
        fixtures::inventory(vec![
            fixtures::vault("kv-contoso-dev"),
            fixtures::vault("kv-contoso-prod"),
        ])
    }

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
                fixtures::inventory(vec![fixtures::vault("kv-contoso-dev")]),
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
            Some("Bearer vault-token-first")
        );
    }

    #[test]
    fn a_next_link_off_the_vault_is_refused_without_sending_the_token() {
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "secret", "list"],
            vec![
                fixtures::inventory(vec![fixtures::vault("kv-contoso-dev")]),
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
        let mut bad = fixtures::vault("kv-contoso-dev");
        bad["vaultUri"] = json!("https://kv-contoso-dev.vault.azure.net.evil.example/");
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "secret", "list"],
            vec![fixtures::inventory(vec![bad])],
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
    fn filters_narrow_by_name_expiry_and_state() {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let answer = Answer::json(&json!({"value": [
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-password", "attributes": {"exp": now + 5 * 86_400}},
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-old", "attributes": {"exp": now - 86_400, "enabled": false}},
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-later", "attributes": {"exp": now + 90 * 86_400}},
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/api-key", "attributes": {}},
        ]}));
        let inventory = || fixtures::inventory(vec![fixtures::vault("kv-contoso-dev")]);
        let names = |argv: &[&str]| {
            let (outcome, _) = azure(&[KV], argv, vec![inventory(), answer.clone()]);
            assert_eq!(outcome.code, 0, "{outcome:?}");
            outcome
                .json()
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&["kv", "secret", "list", "DB", "--expires-within", "30d"]),
            ["db-old", "db-password"]
        );
        assert_eq!(names(&["kv", "secret", "list", "--expired"]), ["db-old"]);
        assert_eq!(names(&["kv", "secret", "list", "--disabled"]), ["db-old"]);
        assert_eq!(
            names(&["kv", "secret", "list", "--limit", "1"]),
            ["api-key"]
        );

        let (outcome, transport) = azure(
            &[KV],
            &["kv", "secret", "list", "--expires-within", "soon"],
            vec![],
        );
        assert_eq!(
            outcome.code, 2,
            "a bad window is caught before anything is sent: {outcome:?}"
        );
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn a_vault_flag_reads_one_vault_and_one_outside_the_allowlist_is_a_usage_error() {
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "secret", "list", "--vault", "KV-CONTOSO-PROD"],
            vec![two_vaults(), listing("kv-contoso-prod", &["db-password"])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(transport.sent().len(), 2, "the inventory and one vault");

        let (outcome, _) = azure_with(
            &[KV],
            &["kv", "secret", "list", "--vault", "kv-contoso-dev"],
            vec![two_vaults()],
            "[azure]\nparallel = 1\nvaults = \"kv-contoso-prod\"\n",
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("[azure] leaves out the vault kv-contoso-dev"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = azure_with(
            &[KV],
            &["kv", "secret", "list"],
            vec![two_vaults()],
            "[azure]\nvaults = [\"kv-gone\"]\n",
        );
        assert_eq!(
            outcome.code, 3,
            "an allowlist that reaches nothing is not an empty list: {outcome:?}"
        );
    }

    #[test]
    fn get_needs_reveal_and_a_name_in_two_vaults_is_ambiguous() {
        let (outcome, transport) = azure(&[KV], &["kv", "secret", "get", "db-password"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("--reveal"), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());

        let (outcome, transport) = azure(
            &[KV],
            &["kv", "secret", "get", "db-password", "--reveal"],
            vec![
                two_vaults(),
                listing("kv-contoso-dev", &["db-password"]),
                listing("kv-contoso-prod", &["DB-PASSWORD"]),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains(
                "db-password is in more than one vault (kv-contoso-dev, kv-contoso-prod)"
            ),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.remaining(), 0);
        assert_eq!(transport.sent().len(), 3, "no value was read");

        let (outcome, _) = azure(
            &[KV],
            &["kv", "secret", "get", "nope", "--reveal"],
            vec![
                two_vaults(),
                listing("kv-contoso-dev", &["a"]),
                listing("kv-contoso-prod", &["b"]),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }

    #[test]
    fn get_reads_the_one_holder_and_names_the_version_it_came_from() {
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "secret", "get", "db-password", "--reveal"],
            vec![
                two_vaults(),
                listing("kv-contoso-dev", &["api-key"]),
                listing("kv-contoso-prod", &["db-password"]),
                Answer::json(&json!({
                    "value": "s3cr3t-fixture-value",
                    "id": "https://kv-contoso-prod.vault.azure.net/secrets/db-password/8f3a2c1d",
                    "contentType": "text/plain",
                })),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let value = outcome.json();
        assert_eq!(value["vault"], "kv-contoso-prod");
        assert_eq!(value["version"], "8f3a2c1d");
        assert_eq!(value["value"], "s3cr3t-fixture-value");
        assert_eq!(
            transport.sent()[3].url,
            "https://kv-contoso-prod.vault.azure.net/secrets/db-password?api-version=7.4"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("value.txt");
        let (outcome, _) = azure(
            &[KV],
            &[
                "kv",
                "secret",
                "get",
                "db-password",
                "--version",
                "1c2b",
                "--fields",
                "value",
                "--output",
                path.to_str().unwrap(),
            ],
            vec![
                fixtures::inventory(vec![fixtures::vault("kv-contoso-dev")]),
                Answer::json(
                    &json!({"value": "old-fixture-value", "id": "https://kv-contoso-dev.vault.azure.net/secrets/db-password/1c2b"}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            !outcome.stdout.contains("old-fixture-value"),
            "{}",
            outcome.stdout
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old-fixture-value");
    }

    #[test]
    fn versions_come_back_newest_first() {
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "version", "list", "db-password"],
            vec![
                fixtures::inventory(vec![fixtures::vault("kv-contoso-dev")]),
                Answer::json(&json!({"value": [
                    {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-password/1c2b", "attributes": {"created": 1_709_251_200}},
                    {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-password/8f3a", "attributes": {"created": 1_725_753_600, "enabled": false}},
                ]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["version"], "8f3a");
        assert_eq!(rows[0]["enabled"], false);
        assert_eq!(rows[1]["version"], "1c2b");
        assert!(
            transport.sent()[1]
                .url
                .contains("/secrets/db-password/versions?api-version=7.4")
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
                    fixtures::inventory(vec![fixtures::vault("kv-contoso-dev")]),
                    Answer::status(403, body),
                ],
            );
            assert_eq!(outcome.code, 1, "{outcome:?}");
            assert!(outcome.stderr.contains(want), "{}", outcome.stderr);
        }
    }

    #[test]
    fn one_vault_that_does_not_answer_is_named_and_the_rest_still_print() {
        let (outcome, _) = azure(
            &[KV],
            &["kv", "secret", "list"],
            vec![
                two_vaults(),
                Answer::status(500, "{}"),
                listing("kv-contoso-prod", &["a"]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["vault"], "kv-contoso-prod");
        assert!(outcome.stderr.contains("[not read: "), "{}", outcome.stderr);
    }

    #[test]
    fn a_throttle_longer_than_the_deadline_fails_now_with_124() {
        let started = std::time::Instant::now();
        let (outcome, _) = azure(
            &[KV],
            &["kv", "vault", "list", "--timeout", "5"],
            vec![Answer::status(429, "{}").with_header("Retry-After", "600")],
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "no sleep into the same failure"
        );
    }

    #[test]
    fn the_inventory_is_cached_and_no_cache_reads_it_again() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_owned();
        let setup = |answers: Vec<Answer>| {
            let transport = agent_cli_core::testing::FakeTransport::answering(answers);
            let mut setup =
                agent_cli_core::Setup::fake(transport.clone()).with_config(fixtures::SERIAL);
            setup.cache_dir = Some(dir.clone());
            (setup, transport)
        };
        let (first, transport) = setup(vec![fixtures::inventory(vec![fixtures::vault(
            "kv-contoso-dev",
        )])]);
        let outcome = agent_cli_core::testing::run(&[KV], &["kv", "vault", "list"], first);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(transport.sent().len(), 1);
        let (second, transport) = setup(vec![]);
        let outcome = agent_cli_core::testing::run(&[KV], &["kv", "vault", "list"], second);
        assert_eq!(outcome.json()[0]["name"], "kv-contoso-dev");
        assert!(transport.sent().is_empty(), "the second run read the cache");
        let (third, transport) = setup(vec![fixtures::inventory(vec![])]);
        let outcome =
            agent_cli_core::testing::run(&[KV], &["kv", "vault", "list", "--no-cache"], third);
        assert_eq!(outcome.stdout.trim(), "[]");
        assert_eq!(transport.sent().len(), 1);
        let cached = std::fs::read_to_string(dir.join("cache.json")).unwrap();
        assert!(!cached.contains("token"), "{cached}");
    }

    #[test]
    fn doctor_checks_the_login_the_inventory_and_each_vault_without_reading_a_value() {
        let (ctx, transport) = fixtures::doctor_ctx(
            vec![
                two_vaults(),
                listing("kv-contoso-dev", &["db-password"]),
                Answer::status(
                    403,
                    r#"{"error":{"code":"Forbidden","message":"Client address is not authorized"}}"#,
                ),
            ],
            "[azure]\nvaults = [\"kv-contoso-dev\", \"kv-contoso-prod\", \"kv-gone\"]\n",
        );
        let checks = doctor(&ctx);
        assert_eq!(
            fixtures::rows(&checks),
            [
                ("az login".to_owned(), true),
                ("vault token".to_owned(), true),
                ("inventory".to_owned(), true),
                ("vault kv-gone".to_owned(), false),
                ("vault kv-contoso-dev".to_owned(), true),
                ("vault kv-contoso-prod".to_owned(), false),
            ]
        );
        assert!(checks[5].detail.contains("firewall"), "{:?}", checks[5]);
        let sent = transport.sent();
        assert!(
            sent[1..]
                .iter()
                .all(|sent| sent.url.ends_with("secrets?api-version=7.4&maxresults=1"))
        );

        let (ctx, _) = fixtures::doctor_ctx(vec![], "");
        assert!(doctor(&ctx).is_empty(), "no [azure], nothing to check");
    }

    #[test]
    fn the_overview_counts_the_allowlist_from_config_alone() {
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(
            status(&config("[azure]\nvaults = [\"a\", \"b\", \"c\"]\n")),
            "kv 3 vaults"
        );
        assert_eq!(status(&config("")), "kv all vaults");
        assert_eq!(
            status(&config("[azure]\nvault = \"typo\"\n")),
            "kv config broken"
        );
        assert_eq!(
            crate::acr::status(&config("[azure]\nregistries = \"contosoacr\"\n")),
            "acr 1 registries"
        );
    }
}
