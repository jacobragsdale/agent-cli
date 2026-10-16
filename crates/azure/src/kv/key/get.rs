//! `kv key get`: one key's bundle (`/keys/{name}[/{version}]`) and its
//! rotation policy (`/keys/{name}/rotationpolicy`). The bundle carries the
//! public key itself (`n`, `e`, `x`, `y`); only its size and curve are read
//! from it, and none of it is printed.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{from_unix, text};
use crate::kv::{
    API_VERSION, base, enabled, found, get, holder, item_ref, last_segment, segment, tags,
};
use crate::net::{list, strings};

#[derive(clap::Args)]
pub struct KeyGetArgs {
    /// The key: its name, its id (vault/name[/version]) or its URI
    key: String,
    /// The vault that holds it; needed when more than one does
    #[arg(long)]
    vault: Option<String>,
    /// A version id; the current one when left out
    #[arg(long)]
    version: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct KeyDetail {
    /// `vault/name/version`.
    id: String,
    vault: String,
    name: String,
    version: Option<String>,
    /// RSA, RSA-HSM, EC, EC-HSM or oct-HSM.
    key_type: Option<String>,
    /// An RSA key's bits.
    size: Option<usize>,
    /// An EC key's curve: P-256, P-384, P-521, P-256K.
    curve: Option<String>,
    /// What it may do: encrypt, decrypt, sign, verify, wrapKey, unwrapKey.
    operations: Vec<String>,
    enabled: bool,
    /// RFC 3339.
    expires: Option<String>,
    not_before: Option<String>,
    created: Option<String>,
    updated: Option<String>,
    managed: bool,
    tags: BTreeMap<String, String>,
    /// Absent when the key has none: it never rotates on its own.
    rotation: Option<Rotation>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Rotation {
    /// How long each new version lives, ISO 8601 (`P2Y`).
    expires_after: Option<String>,
    actions: Vec<Action>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Action {
    /// Rotate or Notify.
    action: Option<String>,
    /// ISO 8601 durations, as the policy writes them.
    after_create: Option<String>,
    before_expiry: Option<String>,
}

fn key_get(ctx: &Ctx, args: KeyGetArgs) -> Result<KeyDetail> {
    let key = item_ref(
        &args.key,
        "keys",
        args.vault.as_deref(),
        args.version.as_deref(),
    )?;
    let vault = holder(ctx, "keys", &key.name, key.vault.as_deref())?;
    let at = format!("{}keys/{}", base(&vault), segment(&key.name));
    let url = match &key.version {
        Some(version) => format!("{at}/{}?api-version={API_VERSION}", segment(version)),
        None => format!("{at}?api-version={API_VERSION}"),
    };
    let answer = get(ctx, &vault, &url)?;
    // VERIFY(work): that a key with no policy answers 404 rather than an
    // empty policy, and that Key Vault Crypto User may read it.
    let policy = match found(
        ctx,
        &vault,
        &format!("{at}/rotationpolicy?api-version={API_VERSION}"),
    ) {
        Ok(policy) => policy,
        Err(error) => {
            ctx.note(format!("[rotation policy not read: {error:#}]"));
            None
        }
    };
    let material = &answer["key"];
    let attributes = &answer["attributes"];
    let version = last_segment(material);
    Ok(KeyDetail {
        id: format!(
            "{}/{}/{}",
            vault.name,
            key.name,
            version.as_deref().unwrap_or_default()
        ),
        vault: vault.name.clone(),
        name: key.name,
        version,
        key_type: text(&material["kty"]),
        size: text(&material["n"]).map(|n| bits(&n)),
        curve: text(&material["crv"]),
        operations: strings(&material["key_ops"]),
        enabled: enabled(attributes),
        expires: from_unix(&attributes["exp"]),
        not_before: from_unix(&attributes["nbf"]),
        created: from_unix(&attributes["created"]),
        updated: from_unix(&attributes["updated"]),
        managed: answer["managed"].as_bool().unwrap_or(false),
        tags: tags(&answer),
        rotation: policy.as_ref().and_then(rotation),
    })
}

/// An RSA modulus's bits from its unpadded base64url length: 342 characters
/// are 256 bytes, 2048 bits.
fn bits(modulus: &str) -> usize {
    modulus.trim_end_matches('=').len() * 6 / 8 * 8
}

/// A policy with neither actions nor an expiry rotates nothing.
fn rotation(policy: &Value) -> Option<Rotation> {
    let actions: Vec<Action> = list(&policy["lifetimeActions"])
        .iter()
        .map(|held| Action {
            action: text(&held["action"]["type"]),
            after_create: text(&held["trigger"]["timeAfterCreate"]),
            before_expiry: text(&held["trigger"]["timeBeforeExpiry"]),
        })
        .collect();
    let expires_after = text(&policy["attributes"]["expiryTime"]);
    (!actions.is_empty() || expires_after.is_some()).then_some(Rotation {
        expires_after,
        actions,
    })
}

command! {
    pub KEY_GET = ["kv", "key", "get"], Read,
    "Show a Key Vault key's type, size, operations, expiry and rotation policy",
    keywords: ["crypto", "encryption", "rsa", "curve", "cmk", "rotate", "rotation", "autorotate"],
    example: "kv key get kv-contoso-prod/orders-cmk --fields key_type,size,rotation",
    run: key_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::testing::{self, azure};

    const KEY: &str = "https://kv-contoso-dev.vault.azure.net/keys/orders-cmk";

    fn bundle() -> Answer {
        Answer::json(&json!({
            "key": {"kid": format!("{KEY}/1a2b"), "kty": "RSA",
                "key_ops": ["wrapKey", "unwrapKey"], "n": "A".repeat(342), "e": "AQAB"},
            "attributes": {"enabled": true, "created": 1_709_251_200, "exp": 1_803_895_200},
            "tags": {"team": "orders"},
        }))
    }

    #[test]
    fn a_key_shows_its_size_and_rotation_and_never_its_material() {
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "key", "get", "kv-contoso-dev/orders-cmk"],
            vec![
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                bundle(),
                Answer::json(&json!({"id": format!("{KEY}/rotationpolicy"),
                    "lifetimeActions": [
                        {"trigger": {"timeAfterCreate": "P90D"}, "action": {"type": "Rotate"}},
                        {"trigger": {"timeBeforeExpiry": "P30D"}, "action": {"type": "Notify"}}],
                    "attributes": {"expiryTime": "P1Y"}})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert_eq!(sent[1].url, format!("{KEY}?api-version=7.4"));
        assert_eq!(sent[2].url, format!("{KEY}/rotationpolicy?api-version=7.4"));
        let row = outcome.json();
        assert_eq!(row["id"], "kv-contoso-dev/orders-cmk/1a2b");
        assert_eq!(row["key_type"], "RSA");
        assert_eq!(row["size"], 2048);
        assert_eq!(row["operations"], json!(["wrapKey", "unwrapKey"]));
        assert_eq!(
            row["rotation"],
            json!({"expires_after": "P1Y", "actions": [
                {"action": "Rotate", "after_create": "P90D"},
                {"action": "Notify", "before_expiry": "P30D"}]})
        );
        assert!(!outcome.stdout.contains("AAAA"), "{}", outcome.stdout);
        assert!(!outcome.stdout.contains("AQAB"), "{}", outcome.stdout);
    }

    #[test]
    fn a_key_with_no_policy_has_no_rotation() {
        let (outcome, _) = azure(
            &[KV],
            &["kv", "key", "get", "orders-cmk", "--version", "1a2b"],
            vec![
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                bundle(),
                Answer::status(404, r#"{"error":{"code":"RotationPolicyNotFound"}}"#),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json().get("rotation"), None);
    }
}
