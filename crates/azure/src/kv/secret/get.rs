//! `kv secret get`.

use agent_cli_core::{Ctx, Secret, command};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::text;
use crate::kv::{API_VERSION, base, get, holder, last_segment, secret_ref, segment};

#[derive(clap::Args)]
pub struct SecretGetArgs {
    /// The secret: its name, its id (vault/name[/version]) or its URI
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
    let secret = secret_ref(&args.name, args.vault.as_deref(), args.version.as_deref())?;
    let args = SecretGetArgs {
        name: secret.name,
        vault: secret.vault,
        version: secret.version,
    };
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

command! {
    pub SECRET_GET = ["kv", "secret", "get"], Reveal,
    "Get a Key Vault secret's value (needs --reveal, or --output FILE)",
    keywords: ["password", "credential", "connection", "string", "value", "read", "fetch"],
    example: "kv secret get kv-contoso-dev/db-password --fields value --output db-password.txt",
    run: secret_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::testing::{self, azure, listing, two_vaults};

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
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
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
}
