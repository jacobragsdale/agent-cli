//! `kv certificate get`: one certificate's bundle (`/certificates/{name}`),
//! with its policy and any pending operation (`/pending`, a 404 when there
//! is none). An expired certificate, or one expiring within 30 days, names
//! its versions next: whether a renewal has landed.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{parse_stamp, text};
use crate::kv::certificate::CertificateRow;
use crate::kv::{API_VERSION, base, found, get, holder, item_ref, last_segment, segment};
use crate::net::{list, strings};

/// What "expiring soon" means for the next note.
const SOON: time::Duration = time::Duration::days(30);

#[derive(clap::Args)]
pub struct CertificateGetArgs {
    /// The certificate: its name, its id (vault/name[/version]) or its URI
    certificate: String,
    /// The vault that holds it; needed when more than one does
    #[arg(long)]
    vault: Option<String>,
    /// A version id; the current one when left out
    #[arg(long)]
    version: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CertificateDetail {
    #[serde(flatten)]
    row: CertificateRow,
    version: Option<String>,
    policy: Policy,
    /// A renewal or creation still under way, or failed.
    pending: Option<Pending>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Policy {
    /// RSA, RSA-HSM, EC or EC-HSM.
    key_type: Option<String>,
    key_size: Option<u64>,
    curve: Option<String>,
    reuse_key: Option<bool>,
    exportable: Option<bool>,
    /// application/x-pkcs12 or application/x-pem-file.
    content_type: Option<String>,
    validity_months: Option<u64>,
    key_usage: Vec<String>,
    /// What happens as it ages: AutoRenew or EmailContacts, at a percentage
    /// of its lifetime or some days before it expires.
    lifetime_actions: Vec<LifetimeAction>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LifetimeAction {
    action: Option<String>,
    lifetime_percentage: Option<u64>,
    days_before_expiry: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Pending {
    /// inProgress, completed, cancelled or failed.
    state: Option<String>,
    details: Option<String>,
    /// `CODE: message`.
    error: Option<String>,
    request_id: Option<String>,
}

fn certificate_get(ctx: &Ctx, args: CertificateGetArgs) -> Result<CertificateDetail> {
    let wanted = item_ref(
        &args.certificate,
        "certificates",
        args.vault.as_deref(),
        args.version.as_deref(),
    )?;
    let vault = holder(ctx, "certificates", &wanted.name, wanted.vault.as_deref())?;
    let at = format!("{}certificates/{}", base(&vault), segment(&wanted.name));
    let url = match &wanted.version {
        Some(version) => format!("{at}/{}?api-version={API_VERSION}", segment(version)),
        None => format!("{at}?api-version={API_VERSION}"),
    };
    let bundle = get(ctx, &vault, &url)?;
    let pending = found(
        ctx,
        &vault,
        &format!("{at}/pending?api-version={API_VERSION}"),
    )?;
    let row = CertificateRow::new(&vault.name, &wanted.name, &bundle);
    if let Some(expires) = row.expires.as_deref().and_then(parse_stamp)
        && expires <= agent_cli_core::now() + SOON
    {
        let how = if row.autorenew {
            "Key Vault should have renewed it by now"
        } else {
            "it does not renew itself"
        };
        ctx.note(format!(
            "[next: agent-cli kv version list {}  (it expires {}, and {how}: a newer version is a renewal)]",
            row.id,
            row.expires.as_deref().unwrap_or_default()
        ));
    }
    Ok(CertificateDetail {
        version: last_segment(&bundle),
        policy: policy(&bundle["policy"]),
        pending: pending.map(|held| Pending {
            state: text(&held["status"]),
            details: text(&held["status_details"]),
            error: text(&held["error"]["code"]).map(|code| match text(&held["error"]["message"]) {
                Some(message) => format!("{code}: {message}"),
                None => code,
            }),
            request_id: text(&held["request_id"]),
        }),
        row,
    })
}

fn policy(policy: &Value) -> Policy {
    let key = &policy["key_props"];
    Policy {
        key_type: text(&key["kty"]),
        key_size: key["key_size"].as_u64(),
        curve: text(&key["crv"]),
        reuse_key: key["reuse_key"].as_bool(),
        exportable: key["exportable"].as_bool(),
        content_type: text(&policy["secret_props"]["contentType"]),
        validity_months: policy["x509_props"]["validity_months"].as_u64(),
        key_usage: strings(&policy["x509_props"]["key_usage"]),
        lifetime_actions: list(&policy["lifetime_actions"])
            .iter()
            .map(|held| LifetimeAction {
                action: text(&held["action"]["action_type"]),
                lifetime_percentage: held["trigger"]["lifetime_percentage"].as_u64(),
                days_before_expiry: held["trigger"]["days_before_expiry"].as_u64(),
            })
            .collect(),
    }
}

command! {
    pub CERTIFICATE_GET = ["kv", "certificate", "get"], Read,
    "Show a Key Vault certificate with its policy and any pending renewal",
    keywords: ["tls", "ssl", "x509", "renewal", "autorenew", "issuer", "san", "policy", "pending"],
    example: "kv certificate get kv-contoso-prod/www-contoso-example --fields subject,expires,autorenew,pending",
    run: certificate_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::testing::{self, azure};

    const AT: &str = "https://kv-contoso-dev.vault.azure.net/certificates/web-tls";

    fn bundle(days: i64, renews: bool) -> Answer {
        let exp = agent_cli_core::now().unix_timestamp() + days * 86_400;
        let action = if renews { "AutoRenew" } else { "EmailContacts" };
        Answer::json(&json!({
            "id": format!("{AT}/77aa"), "x5t": "2jmj7l5rSw0yVb_vlWAYkK_YBwk",
            "attributes": {"enabled": true, "exp": exp},
            "policy": {"issuer": {"name": "Self"},
                "key_props": {"kty": "RSA", "key_size": 2048, "reuse_key": false, "exportable": true},
                "secret_props": {"contentType": "application/x-pkcs12"},
                "x509_props": {"subject": "CN=web", "validity_months": 12, "key_usage": ["digitalSignature"]},
                "lifetime_actions": [{"trigger": {"lifetime_percentage": 80}, "action": {"action_type": action}}]},
        }))
    }

    fn get(answers: Vec<Answer>) -> agent_cli_core::testing::Outcome {
        let mut all = vec![testing::inventory(vec![testing::vault("kv-contoso-dev")])];
        all.extend(answers);
        azure(
            &[KV],
            &["kv", "certificate", "get", "kv-contoso-dev/web-tls"],
            all,
        )
        .0
    }

    #[test]
    fn a_certificate_far_from_expiry_shows_its_policy_and_no_note() {
        let outcome = get(vec![
            bundle(200, true),
            Answer::status(404, r#"{"error":{"code":"PendingCertificateNotFound"}}"#),
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = outcome.json();
        assert_eq!(row["id"], "kv-contoso-dev/web-tls");
        assert_eq!(row["version"], "77aa");
        assert_eq!(row["autorenew"], true);
        assert_eq!(row["policy"]["key_size"], 2048);
        assert_eq!(
            row["policy"]["lifetime_actions"],
            json!([{"action": "AutoRenew", "lifetime_percentage": 80}])
        );
        assert_eq!(row.get("pending"), None);
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);
    }

    #[test]
    fn a_certificate_expiring_soon_names_its_versions_next_and_shows_a_failed_renewal() {
        let outcome = get(vec![
            bundle(9, true),
            Answer::json(
                &json!({"status": "failed", "status_details": "Issuer rejected the request",
                "error": {"code": "CertificateIssuerError", "message": "DigiCert: order declined"},
                "request_id": "r-1"}),
            ),
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["pending"],
            json!({"state": "failed", "details": "Issuer rejected the request",
                "error": "CertificateIssuerError: DigiCert: order declined", "request_id": "r-1"})
        );
        assert_eq!(
            agent_cli_core::testing::next_command(&outcome.stderr),
            Some(vec![
                "kv".to_owned(),
                "version".to_owned(),
                "list".to_owned(),
                "kv-contoso-dev/web-tls".to_owned()
            ])
        );
    }
}
