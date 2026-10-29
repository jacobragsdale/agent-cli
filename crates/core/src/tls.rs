//! How the real transport trusts a server: the OS trust store rather than a
//! bundled one, so a server signed by a company CA (an on-prem Control-M, an
//! internal Airflow) is trusted once the CA is installed the way every other
//! tool on the machine sees it, and a refused certificate names that fix.

use std::hash::{DefaultHasher, Hash, Hasher};

use anyhow::{Context, Result};
use ureq::tls::{Certificate, ClientCert, PemItem, PrivateKey, RootCerts, TlsConfig};

use crate::error::Failure;
use crate::proxy::Via;
use crate::secret::Secret;

/// How one request trusts its server and proves who it is, when it does not
/// take the OS trust store as every domain does (`http --cacert`, `--cert`).
#[derive(Clone, Debug, Default)]
pub struct Tls {
    /// PEM certificates trusted instead of the system store.
    pub roots: Option<Vec<u8>>,
    /// A client certificate chain (PEM) and its key (PEM), for mutual TLS.
    pub client: Option<(Vec<u8>, Secret)>,
    /// Accept any certificate, as curl's `--insecure` does.
    pub insecure: bool,
}

impl Tls {
    /// What tells two settings apart for the agent cache; the key is left
    /// out, since its certificate names it.
    pub(crate) fn key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.roots.hash(&mut hasher);
        self.client
            .as_ref()
            .map(|(chain, _)| chain)
            .hash(&mut hasher);
        self.insecure.hash(&mut hasher);
        hasher.finish()
    }
}

/// The agent a request goes out on: redirects are answers, statuses are not
/// errors, certificates are checked against the OS trust store unless `tls`
/// says otherwise, and the proxy is the one core chose (never ureq's own
/// reading of the environment).
pub(crate) fn agent(via: Option<&Via>, tls: Option<&Tls>) -> Result<ureq::Agent> {
    let mut config = TlsConfig::builder().root_certs(RootCerts::PlatformVerifier);
    if let Some(tls) = tls {
        if let Some(pem) = &tls.roots {
            let roots = certificates(pem).context("--cacert")?;
            config = config.root_certs(RootCerts::new_with_certs(&roots));
        }
        if let Some((chain, key)) = &tls.client {
            let chain = certificates(chain).context("--cert")?;
            let key = PrivateKey::from_pem(key.expose().as_bytes()).map_err(|why| {
                Failure::usage(format!(
                    "--cert holds no PEM private key (one PEM with both, or CERT,KEY) ({why})"
                ))
            })?;
            config = config.client_cert(Some(ClientCert::new_with_certs(&chain, key)));
        }
        config = config.disable_verification(tls.insecure);
    }
    let proxy = via
        .map(|via| {
            ureq::Proxy::new(&via.wire()).map_err(|why| {
                Failure::setup(format!("the proxy {} ({}): {why}", via.url, via.from))
            })
        })
        .transpose()?;
    Ok(ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(proxy)
        .tls_config(config.build())
        .build()
        .into())
}

/// Every certificate in a PEM bundle; none is a mistake worth naming.
fn certificates(pem: &[u8]) -> Result<Vec<Certificate<'static>>> {
    let mut found = Vec::new();
    for item in ureq::tls::parse_pem(pem) {
        if let PemItem::Certificate(certificate) =
            item.map_err(|why| Failure::usage(format!("not PEM ({why})")))?
        {
            found.push(certificate);
        }
    }
    if found.is_empty() {
        return Err(
            Failure::usage("holds no PEM certificate (-----BEGIN CERTIFICATE-----)").into(),
        );
    }
    Ok(found)
}

/// For a server whose certificate chains to no CA the OS trusts: a company CA,
/// or a self-signed server.
const UNTRUSTED: &str = "install the CA that signed it (or the server's own certificate, if self-signed) in the system store: \
     on Ubuntu or WSL, copy the .crt file to /usr/local/share/ca-certificates/ and run sudo update-ca-certificates";
/// rustls refuses a CA's certificate as a server's, trusted or not.
const CA_AS_SERVER: &str = "the server presents a CA certificate as its own, which is refused even when trusted: \
     ask its admins for a server certificate (CA:FALSE), signed by the company CA";
/// No CA installed fixes these; the server's admins do.
const EXPIRED: &str = "the server's certificate has expired, or is not valid yet (check this machine's clock): \
     no CA fixes that; its admins must renew it";
const OTHER_NAME: &str = "the server's certificate names other hosts: call it by a name it holds, or ask its admins for one naming this host";

/// The hint for a refusal of the server's certificate, from rustls's words
/// (ureq hands them back as an I/O error); `None` for any other failure.
pub(crate) fn untrusted(said: &str) -> Option<&'static str> {
    untrusted_hint(said)
}

/// What to do about a certificate this machine refused, from rustls's words
/// (`invalid peer certificate: UnknownIssuer`); `None` when they name no
/// refusal of trust.
#[must_use]
pub fn untrusted_hint(said: &str) -> Option<&'static str> {
    if !said.contains("invalid peer certificate") {
        return None;
    }
    let lower = said.to_ascii_lowercase();
    Some(if said.contains("CaUsedAsEndEntity") {
        CA_AS_SERVER
    } else if lower.contains("expired")
        || lower.contains("not valid yet")
        || lower.contains("notvalidyet")
    {
        EXPIRED
    } else if lower.contains("not valid for name") || lower.contains("notvalidforname") {
        OTHER_NAME
    } else {
        UNTRUSTED
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_certificate_this_machine_does_not_trust_names_how_to_trust_it() {
        assert_eq!(
            untrusted("io: invalid peer certificate: UnknownIssuer"),
            Some(UNTRUSTED)
        );
        assert_eq!(
            untrusted("io: invalid peer certificate: Other(OtherError(CaUsedAsEndEntity))"),
            Some(CA_AS_SERVER)
        );
        assert_eq!(untrusted("io: Connection refused (os error 111)"), None);
        assert_eq!(
            untrusted(
                "io: invalid peer certificate: certificate expired: verification time 1793241378 (UNIX), but certificate is not valid after 1609459200"
            ),
            Some(EXPIRED)
        );
        assert_eq!(
            untrusted(
                "io: invalid peer certificate: certificate not valid for name \"localhost\"; certificate is only valid for DnsName(\"other.contoso.test\")"
            ),
            Some(OTHER_NAME)
        );
    }
}
