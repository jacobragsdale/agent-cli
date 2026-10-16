//! `kv key list`: each vault's `/keys` listing, which carries attributes and
//! tags only; a key's type, size and rotation policy are `kv key get`'s.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Span, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{from_unix, limited};
use crate::config::Azure;
use crate::graph::Vault;
use crate::kv::{
    API_VERSION, PAGE, Wanted, across, base, enabled, expiring, last_segment, pages, tags, vaults,
};

#[derive(clap::Args)]
pub struct KeyListArgs {
    /// Part of the key name, any case; or a key's id (vault/name), exactly
    name: Option<String>,
    /// Only this vault (repeatable; within [azure] vaults)
    #[arg(long)]
    vault: Vec<String>,
    /// Only keys expiring within this long, or already expired
    #[arg(long)]
    expires_within: Option<Span>,
    /// Only keys whose expiry has passed
    #[arg(long)]
    expired: bool,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// One key, as a listing describes it: never key material.
#[derive(Debug, Serialize, JsonSchema)]
pub struct KeyRow {
    /// `vault/name`: what `kv key get` takes.
    id: String,
    vault: String,
    name: String,
    enabled: bool,
    /// RFC 3339.
    expires: Option<String>,
    created: Option<String>,
    updated: Option<String>,
    /// A certificate's key, which the vault manages itself.
    managed: bool,
    tags: BTreeMap<String, String>,
}

fn keys(ctx: &Ctx, vault: &Vault) -> Result<Vec<KeyRow>> {
    let first = format!(
        "{}keys?api-version={API_VERSION}&maxresults={PAGE}",
        base(vault)
    );
    Ok(pages(ctx, vault, first)?
        .iter()
        .filter_map(|entry| {
            let attributes = &entry["attributes"];
            let name = last_segment(entry)?;
            Some(KeyRow {
                id: format!("{}/{name}", vault.name),
                vault: vault.name.clone(),
                name,
                enabled: enabled(attributes),
                expires: from_unix(&attributes["exp"]),
                created: from_unix(&attributes["created"]),
                updated: from_unix(&attributes["updated"]),
                managed: entry["managed"].as_bool().unwrap_or(false),
                tags: tags(entry),
            })
        })
        .collect())
}

fn key_list(ctx: &Ctx, args: KeyListArgs) -> Result<Vec<KeyRow>> {
    let wanted = Wanted::new(args.name.as_deref(), "keys", &args.vault)?;
    let azure = Azure::load(ctx)?;
    let vaults = vaults(ctx, &azure, &wanted.only)?;
    let mut rows: Vec<KeyRow> = across(ctx, &azure, &vaults, |vault| keys(ctx, vault))?
        .into_iter()
        .filter(|row| wanted.matches(&row.name))
        .filter(|row| expiring(row.expires.as_deref(), args.expired, args.expires_within))
        .collect();
    rows.sort_by(|a, b| {
        (&a.vault, a.name.to_ascii_lowercase()).cmp(&(&b.vault, b.name.to_ascii_lowercase()))
    });
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub KEY_LIST = ["kv", "key", "list"], Read,
    "List Key Vault keys with expiry and tags (never key material)",
    keywords: ["crypto", "encryption", "rsa", "cmk", "expiring", "expired", "vaults"],
    example: "kv key list --expires-within 30d --fields id,expires",
    run: key_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::testing::{self, azure};

    #[test]
    fn keys_list_by_kid_and_filter_by_name_and_expiry() {
        let now = agent_cli_core::now().unix_timestamp();
        let answer = Answer::json(&json!({"value": [
            {"kid": "https://kv-contoso-dev.vault.azure.net/keys/orders-cmk", "attributes": {"enabled": true, "exp": now + 10 * 86_400, "created": 1_709_251_200}, "tags": {"team": "orders"}},
            {"kid": "https://kv-contoso-dev.vault.azure.net/keys/sign", "attributes": {"enabled": false}, "managed": true},
        ], "nextLink": null}));
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "key", "list"],
            vec![
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                answer.clone(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent()[1].url,
            "https://kv-contoso-dev.vault.azure.net/keys?api-version=7.4&maxresults=25"
        );
        let rows = outcome.json();
        assert_eq!(rows[0]["id"], "kv-contoso-dev/orders-cmk");
        assert_eq!(rows[0]["created"], "2024-03-01T00:00:00Z");
        assert_eq!(rows[1]["enabled"], false);
        assert_eq!(rows[1]["managed"], true);

        let (outcome, _) = azure(
            &[KV],
            &[
                "kv",
                "key",
                "list",
                "--expires-within",
                "14d",
                "--fields",
                "id",
            ],
            vec![
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                answer,
            ],
        );
        assert_eq!(outcome.json(), json!([{"id": "kv-contoso-dev/orders-cmk"}]));
    }
}
