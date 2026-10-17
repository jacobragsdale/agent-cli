//! `kv vault diff`: the secrets of two vaults (staging's and prod's) by
//! name: those on one side only, each one's enabled, expiry, content type
//! and tags, and each value compared in memory, printed only as `same:
//! true|false`. One GET per secret held by both, side by side, at most
//! `--limit` of them.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Side, command, diff};
use anyhow::Result;
use serde_json::{Map, Value};

use crate::client::parallel;
use crate::config::Azure;
use crate::graph::Vault;
use crate::kv::{API_VERSION, base, compared, last_segment, pages, secret_for_diff, vaults};

#[derive(clap::Args)]
pub struct VaultDiffArgs {
    /// One vault, by name
    left: String,
    /// The other vault, by name
    right: String,
    #[command(flatten)]
    diff: DiffArgs,
}

fn vault_diff(ctx: &Ctx, args: VaultDiffArgs) -> Result<Diff> {
    let azure = Azure::load(ctx)?;
    let reached = vaults(ctx, &azure, &[args.left.clone(), args.right.clone()])?;
    let one = |name: &str| -> Result<(Vault, BTreeMap<String, Value>)> {
        let vault = reached
            .iter()
            .find(|vault| vault.name.eq_ignore_ascii_case(name))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no vault {name} in reach"))?;
        let first = format!(
            "{}secrets?api-version={API_VERSION}&maxresults=25",
            base(&vault)
        );
        let held = pages(ctx, &vault, first)?
            .iter()
            .filter_map(|entry| Some((last_segment(entry)?, compared(entry))))
            .collect();
        Ok((vault, held))
    };
    let (left_vault, left) = one(&args.left)?;
    let (right_vault, right) = one(&args.right)?;
    let both: Vec<String> = left
        .keys()
        .filter(|name| right.contains_key(*name))
        .cloned()
        .collect();
    let read = both.len().min(args.diff.limit);
    if read < both.len() {
        ctx.note(format!(
            "[values of {read} of the {} secrets both hold compared; --limit N]",
            both.len()
        ));
    }
    let values = |vault: &Vault| {
        parallel(&both[..read], azure.parallel(), |name| {
            secret_for_diff(ctx, vault, name).map(|(_, value)| value)
        })
    };
    let side =
        |vault: &Vault, held: BTreeMap<String, Value>, read: Vec<Result<_>>| -> Result<Side> {
            let mut side = Side::new(
                vault.name.clone(),
                Value::Object(held.into_iter().collect::<Map<_, _>>()),
            );
            for (name, value) in both.iter().zip(read) {
                side.secrets.insert(format!("{name}.value"), value?);
            }
            Ok(side)
        };
    let (left_values, right_values) = (values(&left_vault), values(&right_vault));
    diff(
        ctx,
        &args.diff,
        &Rules::PLAIN,
        side(&left_vault, left, left_values)?,
        side(&right_vault, right, right_values)?,
    )
}

command! {
    pub VAULT_DIFF = ["kv", "vault", "diff"], Read,
    "Compare two key vaults: names on one side only, expiry, each value same or not",
    keywords: ["staging", "missing", "secrets", "names", "keyvault"],
    example: "kv vault diff kv-contoso-staging kv-contoso-prod",
    run: vault_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::testing::{SERIAL, azure_with, listing, two_vaults};

    fn value(vault: &str, name: &str, value: &str) -> Answer {
        Answer::json(&json!({
            "value": value,
            "id": format!("https://{vault}.vault.azure.net/secrets/{name}/0a1b"),
            "contentType": "text/plain",
            "tags": {"owner": "platform"},
            "attributes": {"enabled": true},
        }))
    }

    #[test]
    fn two_vaults_differ_by_the_names_one_lacks_and_the_values_that_differ() {
        let (outcome, _) = azure_with(
            &[KV],
            &["kv", "vault", "diff", "kv-contoso-dev", "kv-contoso-prod"],
            vec![
                two_vaults(),
                listing("kv-contoso-dev", &["api-key", "db-password"]),
                listing(
                    "kv-contoso-prod",
                    &["api-key", "db-password", "orders-db-conn"],
                ),
                value("kv-contoso-dev", "api-key", "same-key-123"),
                value("kv-contoso-dev", "db-password", "dev-pass-123"),
                value("kv-contoso-prod", "api-key", "same-key-123"),
                value("kv-contoso-prod", "db-password", "prod-pass-456"),
            ],
            SERIAL,
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["changes"],
            json!([
                {"path": "orders-db-conn", "right": {"enabled": true, "content_type": "text/plain",
                    "managed": false, "tags": {"owner": "platform"}}},
                {"path": "db-password.value", "redacted": true, "same": false},
            ])
        );
        assert!(!outcome.stdout.contains("pass-"), "{}", outcome.stdout);
    }
}
