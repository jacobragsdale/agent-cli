use agent_cli_core::{Ctx, Span, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use super::{CertInfo, Probe, refusal};

#[derive(clap::Args)]
pub struct CertificateListArgs {
    /// Hosts (HOST[:PORT]) or URLs to probe
    #[arg(required = true)]
    targets: Vec<String>,
    /// Only those expiring within this: 30d
    #[arg(long)]
    expires_within: Option<Span>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[command(flatten)]
    probe: Probe,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CertificateRow {
    /// HOST:PORT, what `http certificate get` takes.
    id: String,
    subject: Option<String>,
    issuer: Option<String>,
    not_after: Option<String>,
    days_left: Option<i64>,
    trusted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refusal: Option<String>,
    thumbprint: Option<String>,
    /// Why it could not be probed.
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<String>,
}

fn certificate_list(ctx: &Ctx, args: CertificateListArgs) -> Result<Vec<CertificateRow>> {
    let mut rows: Vec<CertificateRow> = args
        .targets
        .iter()
        .take(args.limit.max(1) * 4)
        .map(|target| match args.probe.shake(ctx, target) {
            Ok((handshake, peer)) => {
                let leaf = CertInfo::of(&peer.chain[0]);
                CertificateRow {
                    id: handshake.target(),
                    subject: leaf.as_ref().map(|l| l.subject.clone()),
                    issuer: leaf.as_ref().map(|l| l.issuer.clone()),
                    not_after: leaf.as_ref().and_then(|l| l.not_after.clone()),
                    days_left: leaf.as_ref().and_then(|l| l.days_left),
                    trusted: Some(peer.refusal.is_none()),
                    refusal: peer
                        .refusal
                        .as_deref()
                        .map(|said| refusal(said, &handshake.host).0),
                    thumbprint: leaf.map(|l| l.thumbprint),
                    problem: None,
                }
            }
            Err(error) => CertificateRow {
                id: target.clone(),
                subject: None,
                issuer: None,
                not_after: None,
                days_left: None,
                trusted: None,
                refusal: None,
                thumbprint: None,
                problem: Some(format!("{error:#}")),
            },
        })
        .collect();
    if let Some(Span(within)) = args.expires_within {
        let days = i64::try_from(within.as_secs() / 86_400).unwrap_or(i64::MAX);
        rows.retain(|row| row.problem.is_some() || row.days_left.is_some_and(|left| left <= days));
    }
    rows.sort_by_key(|row| row.days_left.unwrap_or(i64::MIN));
    let total = rows.len();
    rows.truncate(args.limit);
    if total > rows.len() {
        ctx.note(format!("[{} of {total}; --limit N]", rows.len()));
    }
    Ok(rows)
}

command! {
    pub CERTIFICATE_LIST = ["http", "certificate", "list"], Read,
    "Probe several hosts' TLS certificates, soonest expiry first, or those due soon",
    keywords: ["tls", "ssl", "cert", "expiring", "within", "x509", "hosts", "sites"],
    example: "http certificate list api.contoso.example wiki.contoso.example --expires-within 30d --fields id,not_after,days_left",
    run: certificate_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{FakeTransport, run};
    use agent_cli_core::{Peer, Setup};

    use crate::DOMAIN;
    use crate::testing::leaf_der;

    #[test]
    fn hosts_are_listed_soonest_expiry_first_and_a_failed_probe_says_why() {
        let transport = FakeTransport::default().with_peers([Peer {
            chain: vec![leaf_der()],
            ..Peer::default()
        }]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "certificate",
                "list",
                "api.contoso.test",
                "down.contoso.test",
            ],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["id"], "down.contoso.test");
        assert!(
            rows[0]["problem"]
                .as_str()
                .unwrap()
                .contains("ran out of handshakes")
        );
        assert_eq!(
            rows[1]["thumbprint"],
            "E4298A2276E4835F9C116E220E8507CA074A2A83"
        );
    }
}
