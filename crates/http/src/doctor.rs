//! The overview's line for http, and `agent-cli doctor http`: the proxy
//! requests go through, and whether TLS is intercepted on the way.

use agent_cli_core::{Check, Config, Ctx, Handshake};
use serde::Deserialize;

use crate::certificate::CertInfo;

/// A public host whose certificate a TLS-intercepting proxy replaces.
const PROBE: &str = "login.microsoftonline.com";
/// Words in the issuer that host's own certificates carry.
const ITS_ISSUERS: [&str; 2] = ["Microsoft", "DigiCert"];

#[derive(Default, Deserialize)]
struct Core {
    proxy: Option<String>,
}

/// `http`, or `http via HOST` when `[core] proxy` names one: from config
/// alone, so the environment's proxy is doctor's to name.
pub(crate) fn status(config: &Config) -> String {
    let proxy = config
        .section::<Core>("core")
        .ok()
        .and_then(|core| core.proxy);
    match proxy.filter(|proxy| !proxy.eq_ignore_ascii_case("none")) {
        Some(proxy) => {
            let host = proxy
                .split_once("://")
                .map_or(proxy.as_str(), |(_, rest)| rest);
            format!("http via {}", host.split([':', '/']).next().unwrap_or(host))
        }
        None => "http".to_owned(),
    }
}

/// Which proxy a request goes through, then one handshake with a public
/// host through it: whether the chain is the host's own, or a proxy's CA
/// re-signing it, and whether this machine trusts that CA.
pub(crate) fn doctor(ctx: &Ctx) -> Vec<Check> {
    let mut checks = Vec::new();
    let via = ctx.via(&format!("https://{PROBE}/"));
    match &via {
        Ok(Some(via)) => checks.push(Check::ok("proxy", format!("{} from {}", via.url, via.from))),
        Ok(None) => checks.push(Check::ok("proxy", "none: requests go out directly")),
        Err(error) => {
            checks.push(Check::failed(
                "proxy",
                format!("{error:#}"),
                "fix [core] proxy (or HTTPS_PROXY); `agent-cli config example` shows the keys",
            ));
            return checks;
        }
    }
    checks.push(match ctx.read(Handshake::new(PROBE, 443)) {
        Ok(peer) => {
            let issuer = peer.chain.first().and_then(|der| CertInfo::of(der)).map(|leaf| leaf.issuer().to_owned()).unwrap_or_default();
            let own = ITS_ISSUERS.iter().any(|word| issuer.contains(word));
            match (&peer.refusal, own) {
                (None, true) => Check::ok("tls", format!("{PROBE} is served under {issuer}: not intercepted")),
                (None, false) => Check::ok(
                    "tls",
                    format!("TLS is intercepted: {PROBE} is served under {issuer}, which this machine trusts"),
                ),
                (Some(said), _) => Check::failed(
                    "tls",
                    format!("{PROBE} is served under {issuer}, which this machine does not trust ({said})"),
                    agent_cli_core::untrusted_hint(said).unwrap_or("install the CA that signs it in the system store"),
                ),
            }
        }
        Err(error) => Check::failed(
            "tls",
            format!("no TLS handshake with {PROBE}: {error:#}"),
            "check the network and the proxy: agent-cli http certificate get login.microsoftonline.com",
        ),
    });
    checks
}
