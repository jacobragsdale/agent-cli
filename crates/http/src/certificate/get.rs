use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use super::{CertInfo, Probe, refusal};

#[derive(clap::Args)]
pub struct CertificateGetArgs {
    /// HOST[:PORT] (443 by default) or a URL
    target: String,
    #[command(flatten)]
    probe: Probe,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CertificateDetail {
    /// HOST:PORT.
    id: String,
    /// This machine (or --cacert) trusts the chain for this host.
    trusted: bool,
    /// Why it does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    refusal: Option<String>,
    /// TLSv1_3, TLSv1_2.
    version: Option<String>,
    alpn: Option<String>,
    connect_ms: u64,
    handshake_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy: Option<String>,
    leaf: Option<CertInfo>,
    /// The rest of the chain as sent, nearest the leaf first.
    chain: Vec<CertInfo>,
}

fn certificate_get(ctx: &Ctx, args: CertificateGetArgs) -> Result<CertificateDetail> {
    let (handshake, peer) = args.probe.shake(ctx, &args.target)?;
    let leaf = CertInfo::of(&peer.chain[0]);
    let refused = peer
        .refusal
        .as_deref()
        .map(|said| refusal(said, &handshake.host));
    if let Some((words, hint)) = &refused {
        let hint = hint.map_or_else(String::new, |hint| format!("; {hint}"));
        ctx.note(format!("[this machine does not trust it: {words}{hint}]"));
    }
    if let Some(days) = leaf.as_ref().and_then(|leaf| leaf.days_left)
        && (0..30).contains(&days)
    {
        ctx.note(format!("[it expires in {days} days]"));
    }
    Ok(CertificateDetail {
        id: handshake.target(),
        trusted: peer.refusal.is_none(),
        refusal: refused.map(|(words, _)| words),
        version: peer.version.clone(),
        alpn: peer.alpn.clone(),
        connect_ms: peer.connect_ms,
        handshake_ms: peer.handshake_ms,
        proxy: handshake
            .via
            .as_ref()
            .map(|via| format!("{} ({})", via.url, via.from)),
        leaf,
        chain: peer.chain[1..]
            .iter()
            .filter_map(|der| CertInfo::of(der))
            .collect(),
    })
}

command! {
    pub CERTIFICATE_GET = ["http", "certificate", "get"], Read,
    "Show the TLS certificate a server presents: names, expiry, chain, trusted here",
    keywords: ["tls", "ssl", "cert", "x509", "expire", "expiry", "https", "chain", "trust", "hostname"],
    example: "http certificate get api.contoso.example",
    run: certificate_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{FakeTransport, run};
    use agent_cli_core::{Peer, Setup};

    use crate::DOMAIN;
    use crate::testing::{ca_der, leaf_der};

    #[test]
    fn a_trusted_chain_shows_its_leaf_and_the_rest() {
        let transport = FakeTransport::default().with_peers([Peer {
            chain: vec![leaf_der(), ca_der()],
            version: Some("TLSv1_3".into()),
            alpn: Some("h2".into()),
            ..Peer::default()
        }]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "certificate",
                "get",
                "https://api.contoso.test:58443/x",
            ],
            Setup::fake(transport.clone()),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let detail = outcome.json();
        assert_eq!(detail["id"], "api.contoso.test:58443");
        assert_eq!(detail["trusted"], true);
        assert_eq!(
            detail["leaf"]["sans"],
            serde_json::json!(["localhost", "api.contoso.test", "127.0.0.1"])
        );
        assert_eq!(detail["leaf"]["not_after"], "2036-01-01T00:00:00Z");
        assert_eq!(
            detail["chain"][0]["subject"],
            "CN=Contoso Test Root CA, O=Contoso"
        );
        assert_eq!(detail["chain"][0]["ca"], true);
        assert_eq!(
            transport.handshakes(),
            [("api.contoso.test:58443".to_owned(), None)]
        );
    }

    #[test]
    fn an_untrusted_chain_says_why_and_how_to_trust_it() {
        let transport = FakeTransport::default().with_peers([Peer {
            chain: vec![leaf_der()],
            refusal: Some("invalid peer certificate: UnknownIssuer".into()),
            ..Peer::default()
        }]);
        let outcome = run(
            &[DOMAIN],
            &["http", "certificate", "get", "localhost:58446"],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["trusted"], false);
        assert_eq!(
            outcome.json()["refusal"],
            "it is signed by a CA this machine does not trust"
        );
        assert!(
            outcome.stderr.contains("update-ca-certificates"),
            "{}",
            outcome.stderr
        );
    }
}
