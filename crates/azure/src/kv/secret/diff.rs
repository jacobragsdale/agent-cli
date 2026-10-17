//! `kv secret diff`: two Key Vault secrets (one name in two vaults) by
//! enabled, expiry, content type and tags, and their values compared in
//! memory: printed only as `same: true|false`, never a value, hash or
//! length. A `Read`: `kv secret get` stays the only command that reveals.

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Side, command, diff};
use anyhow::Result;

use crate::kv::{holder, item_ref, secret_for_diff};

#[derive(clap::Args)]
pub struct SecretDiffArgs {
    /// One side: the secret's id (vault/name) or URI
    left: String,
    /// The other side, as left
    right: String,
    #[command(flatten)]
    diff: DiffArgs,
}

fn secret_diff(ctx: &Ctx, args: SecretDiffArgs) -> Result<Diff> {
    let side = |raw: &str| -> Result<Side> {
        let wanted = item_ref(raw, "secrets", None, None)?;
        let vault = holder(ctx, "secrets", &wanted.name, wanted.vault.as_deref())?;
        let (row, value) = secret_for_diff(ctx, &vault, &wanted.name)?;
        let mut side = Side::new(format!("{}/{}", vault.name, wanted.name), row);
        side.secrets.insert("value".to_owned(), value);
        Ok(side)
    };
    let (left, right) = (side(&args.left)?, side(&args.right)?);
    diff(ctx, &args.diff, &Rules::PLAIN, left, right)
}

command! {
    pub SECRET_DIFF = ["kv", "secret", "diff"], Read,
    "Compare two Key Vault secrets: expiry, enabled, value same or not (never shown)",
    keywords: ["staging", "password", "rotated", "same", "expired", "credential", "revealing"],
    example: "kv secret diff kv-contoso-staging/worker-db-password kv-contoso-prod/worker-db-password",
    run: secret_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::testing::{azure, two_vaults};

    fn secret(vault: &str, value: &str, exp: i64) -> Answer {
        Answer::json(&json!({
            "value": value,
            "id": format!("https://{vault}.vault.azure.net/secrets/db-password/0a1b"),
            "contentType": "text/plain",
            "attributes": {"enabled": true, "exp": exp, "created": 1_709_251_200, "updated": 1_725_753_600},
            "tags": {"owner": "platform"},
        }))
    }

    #[test]
    fn two_secrets_differ_by_expiry_and_value_which_is_never_printed() {
        let (outcome, transport) = azure(
            &[KV],
            &[
                "kv",
                "secret",
                "diff",
                "kv-contoso-dev/db-password",
                "kv-contoso-prod/db-password",
            ],
            vec![
                two_vaults(),
                secret("kv-contoso-dev", "dev-value-123", 1_798_761_600),
                two_vaults(),
                secret("kv-contoso-prod", "prod-value-456", 1_760_400_000),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"left": "kv-contoso-dev/db-password", "right": "kv-contoso-prod/db-password",
            "same": false, "as": ["dev=prod"], "changes": [
                {"path": "expires", "left": "2027-01-01T00:00:00Z", "right": "2025-10-14T00:00:00Z"},
                {"path": "value", "redacted": true, "same": false},
            ]})
        );
        assert!(!outcome.stdout.contains("value-"), "{}", outcome.stdout);
        assert_eq!(
            transport.sent()[1].url,
            "https://kv-contoso-dev.vault.azure.net/secrets/db-password?api-version=7.4"
        );
    }
}
