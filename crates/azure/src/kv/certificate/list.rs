//! `kv certificate list`: each vault's `/certificates` listing, narrowed by
//! name and expiry, then each certificate it prints read whole for its
//! subject, SANs, issuer and renewal.

use agent_cli_core::{Ctx, Span, command};
use anyhow::Result;
use serde_json::Value;

use crate::client::{from_unix, limited, parallel};
use crate::config::Azure;
use crate::graph::Vault;
use crate::kv::certificate::{CertificateRow, name_in};
use crate::kv::{API_VERSION, PAGE, Wanted, across, base, expiring, get, pages, segment, vaults};

#[derive(clap::Args)]
pub struct CertificateListArgs {
    /// Part of the certificate name, any case; or a certificate's id (vault/name), exactly
    name: Option<String>,
    /// Only this vault (repeatable; within [azure] vaults)
    #[arg(long)]
    vault: Vec<String>,
    /// Only certificates expiring within this long, or already expired
    #[arg(long)]
    expires_within: Option<Span>,
    /// Only certificates whose expiry has passed
    #[arg(long)]
    expired: bool,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn certificate_list(ctx: &Ctx, args: CertificateListArgs) -> Result<Vec<CertificateRow>> {
    let wanted = Wanted::new(args.name.as_deref(), "certificates", &args.vault)?;
    let azure = Azure::load(ctx)?;
    let vaults = vaults(ctx, &azure, &wanted.only)?;
    let mut entries: Vec<(Vault, String, Value)> = across(ctx, &azure, &vaults, |vault| {
        let first = format!(
            "{}certificates?api-version={API_VERSION}&maxresults={PAGE}",
            base(vault)
        );
        Ok(pages(ctx, vault, first)?
            .into_iter()
            .filter_map(|entry| Some((vault.clone(), name_in(&entry)?, entry)))
            .collect())
    })?
    .into_iter()
    .filter(|(_, name, entry)| {
        wanted.matches(name)
            && expiring(
                from_unix(&entry["attributes"]["exp"]).as_deref(),
                args.expired,
                args.expires_within,
            )
    })
    .collect();
    entries.sort_by(|a, b| {
        (&a.0.name, a.1.to_ascii_lowercase()).cmp(&(&b.0.name, b.1.to_ascii_lowercase()))
    });
    let entries = limited(ctx, entries, args.limit);
    let bundles = parallel(&entries, azure.parallel(), |(vault, name, _)| {
        let url = format!(
            "{}certificates/{}?api-version={API_VERSION}",
            base(vault),
            segment(name)
        );
        get(ctx, vault, &url)
    });
    Ok(entries
        .iter()
        .zip(bundles)
        .map(|((vault, name, entry), bundle)| match bundle {
            Ok(bundle) => CertificateRow::new(&vault.name, name, &bundle),
            Err(error) => {
                ctx.note(format!(
                    "[{}/{name} read from its listing only: {error:#}]",
                    vault.name
                ));
                CertificateRow::new(&vault.name, name, entry)
            }
        })
        .collect())
}

command! {
    pub CERTIFICATE_LIST = ["kv", "certificate", "list"], Read,
    "List Key Vault certificates with subject, SANs, issuer, expiry and autorenew",
    keywords: ["tls", "ssl", "x509", "expiring", "expired", "thumbprint", "domain"],
    example: "kv certificate list --expires-within 30d --fields id,subject,expires,autorenew",
    run: certificate_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::testing::{self, azure};

    const AT: &str = "https://kv-contoso-dev.vault.azure.net/certificates";

    #[test]
    fn certificates_expiring_soon_are_read_whole() {
        let now = agent_cli_core::now().unix_timestamp();
        let (outcome, transport) = azure(
            &[KV],
            &["kv", "certificate", "list", "--expires-within", "14d"],
            vec![
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                Answer::json(&json!({"value": [
                    {"id": format!("{AT}/web-tls"), "x5t": "2jmj7l5rSw0yVb_vlWAYkK_YBwk",
                        "attributes": {"enabled": true, "exp": now + 9 * 86_400}},
                    {"id": format!("{AT}/later"), "attributes": {"exp": now + 200 * 86_400}},
                ], "nextLink": null})),
                Answer::json(&json!({
                    "id": format!("{AT}/web-tls/77aa"), "x5t": "2jmj7l5rSw0yVb_vlWAYkK_YBwk",
                    "cer": "MIIfixture", "attributes": {"enabled": true, "exp": now + 9 * 86_400},
                    "policy": {"issuer": {"name": "Unknown"},
                        "x509_props": {"subject": "CN=www.contoso.example",
                            "sans": {"dns_names": ["www.contoso.example", "contoso.example"]}},
                        "lifetime_actions": [{"trigger": {"days_before_expiry": 30}, "action": {"action_type": "EmailContacts"}}]},
                })),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent()[2].url,
            format!("{AT}/web-tls?api-version=7.4")
        );
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], "kv-contoso-dev/web-tls");
        assert_eq!(rows[0]["subject"], "CN=www.contoso.example");
        assert_eq!(
            rows[0]["sans"],
            json!(["www.contoso.example", "contoso.example"])
        );
        assert_eq!(rows[0]["issuer"], "Unknown");
        assert_eq!(
            rows[0]["thumbprint"],
            "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709"
        );
        assert_eq!(rows[0]["autorenew"], false);
        assert!(!outcome.stdout.contains("MIIfixture"));
    }

    #[test]
    fn a_bundle_that_cannot_be_read_leaves_the_listing_row_and_a_note() {
        let (outcome, _) = azure(
            &[KV],
            &["kv", "certificate", "list"],
            vec![
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                Answer::json(
                    &json!({"value": [{"id": format!("{AT}/web-tls"), "attributes": {}}]}),
                ),
                Answer::status(500, "{}"),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["id"], "kv-contoso-dev/web-tls");
        assert!(
            outcome.stderr.contains("read from its listing only"),
            "{}",
            outcome.stderr
        );
    }
}
