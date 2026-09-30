//! `kv version list`.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{from_unix, limited};
use crate::kv::{API_VERSION, PAGE, base, holder, last_segment, pages, secret_ref, segment};

#[derive(clap::Args)]
pub struct VersionListArgs {
    /// The secret: its name, its id (vault/name) or its URI
    secret: String,
    /// The vault that holds it; needed when more than one does
    #[arg(long)]
    vault: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct VersionRow {
    /// `vault/name/version`: what `kv secret get` takes for this version.
    id: String,
    version: String,
    enabled: bool,
    /// RFC 3339.
    created: Option<String>,
    updated: Option<String>,
    expires: Option<String>,
}

fn version_list(ctx: &Ctx, args: VersionListArgs) -> Result<Vec<VersionRow>> {
    let secret = secret_ref(&args.secret, args.vault.as_deref(), None)?;
    if secret.version.is_some() {
        return Err(Failure::usage(format!(
            "{} names one version; version list takes the secret (vault/name)",
            args.secret
        ))
        .into());
    }
    let vault = holder(ctx, &secret.name, secret.vault.as_deref())?;
    let first = format!(
        "{}secrets/{}/versions?api-version={API_VERSION}&maxresults={PAGE}",
        base(&vault),
        segment(&secret.name)
    );
    let mut rows: Vec<VersionRow> = pages(ctx, &vault, first)?
        .iter()
        .filter_map(|entry| {
            let attributes = &entry["attributes"];
            let version = last_segment(entry)?;
            Some(VersionRow {
                id: format!("{}/{}/{version}", vault.name, secret.name),
                version,
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
    example: "kv version list kv-contoso-dev/db-password --fields id,created,enabled",
    run: version_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::testing::{self, azure};

    #[test]
    fn versions_come_back_newest_first() {
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "version", "list", "db-password"],
            vec![
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                Answer::json(&json!({"value": [
                    {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-password/1c2b", "attributes": {"created": 1_709_251_200}},
                    {"id": "https://kv-contoso-dev.vault.azure.net/secrets/db-password/8f3a", "attributes": {"created": 1_725_753_600, "enabled": false}},
                ]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["id"], "kv-contoso-dev/db-password/8f3a");
        assert_eq!(rows[0]["version"], "8f3a");
        assert_eq!(rows[0]["enabled"], false);
        assert_eq!(rows[1]["version"], "1c2b");
        assert!(
            transport.sent()[1]
                .url
                .contains("/secrets/db-password/versions?api-version=7.4")
        );
    }
}
