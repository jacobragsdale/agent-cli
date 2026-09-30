//! `kv secret list`.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Span, command};
use anyhow::Result;

use crate::client::{limited, narrow, parallel};
use crate::config::Azure;
use crate::graph::{Vault, inventory};
use crate::kv::{SecretRow, VAULTS, secret_ref, secrets};

#[derive(clap::Args)]
pub struct SecretListArgs {
    /// Part of the secret name, any case; or a secret's id (vault/name), exactly
    name: Option<String>,
    /// Only this vault (repeatable; within [azure] vaults)
    #[arg(long)]
    vault: Vec<String>,
    /// Only secrets expiring within this long, or already expired
    #[arg(long)]
    expires_within: Option<Span>,
    /// Only secrets whose expiry has passed
    #[arg(long)]
    expired: bool,
    /// Only disabled secrets
    #[arg(long)]
    disabled: bool,
    /// Only secrets tagged key, or key=value, any case (repeatable)
    #[arg(long)]
    tag: Vec<String>,
    /// Only secrets whose content type contains this, any case
    #[arg(long)]
    content_type: Option<String>,
    /// True for certificates' backing secrets only, false for none of them
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    managed: Option<bool>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// Whether `tags` holds `wanted`: `key` or `key=value`, both any case.
fn tagged(tags: &BTreeMap<String, String>, wanted: &str) -> bool {
    let (key, value) = match wanted.split_once('=') {
        Some((key, value)) => (key.trim(), Some(value.trim())),
        None => (wanted.trim(), None),
    };
    tags.iter().any(|(held, held_value)| {
        held.eq_ignore_ascii_case(key)
            && value.is_none_or(|value| held_value.eq_ignore_ascii_case(value))
    })
}

fn secret_list(ctx: &Ctx, args: SecretListArgs) -> Result<Vec<SecretRow>> {
    let within = args.expires_within.map(|span| span.0);
    let content_type = args.content_type.as_deref().map(str::to_ascii_lowercase);
    // A secret's id reads its one vault and matches its name exactly.
    let (only, exact) = match args.name.as_deref().map(|raw| secret_ref(raw, None, None)) {
        Some(Ok(found)) if found.vault.is_some() => (found.vault.into_iter().collect(), true),
        Some(Err(error)) => return Err(error),
        _ => (args.vault.clone(), false),
    };
    let wanted = match args.name.as_deref() {
        Some(raw) if exact => Some(secret_ref(raw, None, None)?.name.to_ascii_lowercase()),
        other => other.map(str::to_ascii_lowercase),
    };
    let azure = Azure::load(ctx)?;
    let vaults = narrow(
        &inventory(ctx, &azure)?.vaults,
        &azure.vaults,
        &only,
        VAULTS,
        |v| &v.name,
    )?;
    let rows = read_all(ctx, &azure, &vaults)?;
    let now = agent_cli_core::now();
    let expiry = |row: &SecretRow| row.expires.as_deref().and_then(crate::client::parse_stamp);
    let rows = rows
        .into_iter()
        .filter(|row| {
            wanted.as_deref().is_none_or(|wanted| {
                let name = row.name.to_ascii_lowercase();
                if exact {
                    name == wanted
                } else {
                    name.contains(wanted)
                }
            })
        })
        .filter(|row| !args.disabled || !row.enabled)
        .filter(|row| args.tag.iter().all(|tag| tagged(&row.tags, tag)))
        .filter(|row| {
            content_type.as_deref().is_none_or(|wanted| {
                row.content_type
                    .as_deref()
                    .is_some_and(|held| held.to_ascii_lowercase().contains(wanted))
            })
        })
        .filter(|row| args.managed.is_none_or(|managed| row.managed == managed))
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
    keywords: ["password", "credential", "expiring", "expired", "disabled", "vaults", "contains", "find", "tagged", "certificate"],
    example: "kv secret list db --expires-within 30d --fields id,expires",
    run: secret_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::testing::{self, azure, azure_with, listing, two_vaults};

    #[test]
    fn filters_narrow_by_name_expiry_and_state() {
        let now = agent_cli_core::now().unix_timestamp();
        let answer = Answer::json(&json!({"value": [
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-password", "attributes": {"exp": now + 5 * 86_400}},
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-old", "attributes": {"exp": now - 86_400, "enabled": false}},
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-later", "attributes": {"exp": now + 90 * 86_400}},
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/api-key", "attributes": {}},
        ]}));
        let inventory = || testing::inventory(vec![testing::vault("kv-contoso-dev")]);
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
    fn tags_content_type_and_managed_narrow_the_listing_here() {
        let answer = Answer::json(&json!({"value": [
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-password", "contentType": "text/plain",
                "tags": {"Team": "Platform", "rotates": "180d"}, "attributes": {}},
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/api-key", "contentType": "text/plain",
                "tags": {"team": "api"}, "attributes": {}},
            {"id": "https://kv-contoso-dev.vault.azure.net/secrets/web-tls", "contentType": "application/x-pkcs12",
                "managed": true, "tags": {"team": "platform"}, "attributes": {}},
        ]}));
        let names = |argv: &[&str]| {
            let (outcome, transport) = azure(
                &[KV],
                argv,
                vec![
                    testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                    answer.clone(),
                ],
            );
            assert_eq!(outcome.code, 0, "{outcome:?}");
            assert_eq!(
                transport.sent()[1].url,
                "https://kv-contoso-dev.vault.azure.net/secrets?api-version=7.4&maxresults=25",
                "Key Vault filters nothing itself: the listing is read whole"
            );
            outcome
                .json()
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        let list = |extra: &[&str]| names(&[&["kv", "secret", "list"][..], extra].concat());
        assert_eq!(
            list(&["--tag", "team=platform"]),
            ["db-password", "web-tls"]
        );
        assert_eq!(
            list(&["--tag", "team=platform", "--tag", "rotates"]),
            ["db-password"]
        );
        assert_eq!(list(&["--tag", "owner"]), Vec::<String>::new());
        assert_eq!(list(&["--content-type", "PKCS12"]), ["web-tls"]);
        assert_eq!(list(&["--managed", "false"]), ["api-key", "db-password"]);
        assert_eq!(list(&["--managed"]), ["web-tls"]);
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
}
