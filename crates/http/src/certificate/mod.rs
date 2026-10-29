//! `http certificate`: the chain a server presents, read through a TLS
//! handshake that reports rather than refuses.

pub(crate) mod get;
pub(crate) mod list;

use std::path::PathBuf;

use agent_cli_core::{Ctx, Failure, Handshake, Peer, now, untrusted_hint, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::der;
use crate::url;

#[derive(Debug, Serialize, JsonSchema)]
pub struct CertInfo {
    subject: String,
    issuer: String,
    /// Its DNS names and addresses.
    sans: Vec<String>,
    not_before: Option<String>,
    not_after: Option<String>,
    days_left: Option<i64>,
    serial: String,
    /// SHA-1 of the DER in hex, as `agent-cli kv certificate get` prints it.
    thumbprint: String,
    key: String,
    signature: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    ca: bool,
}

impl CertInfo {
    pub(crate) fn issuer(&self) -> &str {
        &self.issuer
    }

    pub(crate) fn of(der: &[u8]) -> Option<Self> {
        let cert = der::parse(der)?;
        let mut sans = cert.dns_names.clone();
        sans.extend(cert.ip_addresses.clone());
        Some(Self {
            days_left: cert.not_after.map(|end| (end - now()).whole_days()),
            not_before: cert.not_before.map(utc_time),
            not_after: cert.not_after.map(utc_time),
            subject: cert.subject,
            issuer: cert.issuer,
            sans,
            serial: cert.serial,
            thumbprint: cert.thumbprint,
            key: cert.key,
            signature: cert.signature,
            ca: cert.is_ca,
        })
    }
}

/// The flags a probe takes. The proxy is core's, as for a request.
#[derive(clap::Args, Clone, Debug, Default)]
pub(crate) struct Probe {
    /// Trust these CAs (PEM) instead of the system store
    #[arg(long)]
    pub(crate) cacert: Option<PathBuf>,
}

impl Probe {
    /// The handshake with `target` (`HOST[:PORT]` or a URL).
    pub(crate) fn shake(&self, ctx: &Ctx, target: &str) -> Result<(Handshake, Peer)> {
        let (host, port) = url::host_and_port(target)?;
        let mut handshake = Handshake::new(host, port);
        if let Some(path) = &self.cacert {
            handshake.roots =
                Some(std::fs::read(path).map_err(|why| {
                    Failure::usage(format!("--cacert {}: {why}", path.display()))
                })?);
        }
        handshake.via = ctx.via(&format!("https://{}/", handshake.target()))?;
        let peer = ctx.read(handshake.clone())?;
        if peer.chain.is_empty() {
            return Err(Failure::new(
                agent_cli_core::Exit::Failed,
                format!("{} presented no certificate", handshake.target()),
            )
            .into());
        }
        Ok((handshake, peer))
    }
}

/// rustls's refusal in words, and what to do about it.
pub(crate) fn refusal(said: &str, host: &str) -> (String, Option<&'static str>) {
    // rustls names some refusals by their variant (UnknownIssuer) and some in
    // words (certificate expired: …), so both are read.
    let lower = said.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    let words = if has(&["expired"]) {
        "it has expired".to_owned()
    } else if has(&["notvalidyet", "not valid yet"]) {
        "it is not valid yet".to_owned()
    } else if has(&["notvalidforname", "not valid for name"]) {
        format!("it does not name {host}")
    } else if has(&["causedasendentity"]) {
        "the server presents a CA's certificate as its own".to_owned()
    } else if has(&["unknownissuer", "unknown issuer"]) {
        "it is signed by a CA this machine does not trust".to_owned()
    } else if has(&["badsignature", "bad signature"]) {
        "its signature does not verify".to_owned()
    } else if has(&["revoked"]) {
        "it is revoked".to_owned()
    } else {
        said.to_owned()
    };
    let hint = untrusted_hint(said);
    (words, hint)
}
