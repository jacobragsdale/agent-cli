//! How the real transport trusts a server: the OS trust store rather than a
//! bundled one, so a server signed by a company CA (an on-prem Control-M, an
//! internal Airflow) is trusted once the CA is installed the way every other
//! tool on the machine sees it, and a refused certificate names that fix.

use ureq::tls::{RootCerts, TlsConfig};

/// The agent every request goes out on: redirects are answers, statuses are
/// not errors, and certificates are checked against the OS trust store.
pub(crate) fn agent() -> ureq::Agent {
    let roots = TlsConfig::builder()
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .tls_config(roots)
        .build()
        .into()
}

/// For a server whose certificate chains to no CA the OS trusts: a company CA,
/// or a self-signed server.
const UNTRUSTED: &str = "install the CA that signed it (or the server's own certificate, if self-signed) in the system store: \
     on Ubuntu or WSL, copy the .crt file to /usr/local/share/ca-certificates/ and run sudo update-ca-certificates";
/// rustls refuses a CA's certificate as a server's, trusted or not.
const CA_AS_SERVER: &str = "the server presents a CA certificate as its own, which is refused even when trusted: \
     ask its admins for a server certificate (CA:FALSE), signed by the company CA";

/// The hint for a refusal of the server's certificate, from rustls's words
/// (ureq hands them back as an I/O error); `None` for any other failure.
pub(crate) fn untrusted(said: &str) -> Option<&'static str> {
    if !said.contains("invalid peer certificate") {
        return None;
    }
    Some(if said.contains("CaUsedAsEndEntity") {
        CA_AS_SERVER
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
    }
}
