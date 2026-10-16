//! What `kv certificate list` and `get` share: a row read from a certificate's
//! bundle (`/certificates/{name}`), whose policy holds the subject, SANs,
//! issuer and whether it renews itself. The listing alone carries none of
//! those, so `list` reads each bundle it prints. Neither prints `cer`, the
//! certificate's bytes.

pub(crate) mod get;
pub(crate) mod list;

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{from_unix, text};
use crate::kv::{enabled, tags};
use crate::net::{list, strings};

#[derive(Debug, Serialize, JsonSchema)]
pub struct CertificateRow {
    /// `vault/name`: what `kv certificate get` takes.
    id: String,
    vault: String,
    name: String,
    /// From the policy: what the next renewal will ask for.
    // VERIFY(work): that an imported certificate's policy holds its own
    // subject and SANs, as the service documents.
    subject: Option<String>,
    sans: Vec<String>,
    /// Self, Unknown (renewed by hand, outside Key Vault) or the CA's name.
    issuer: Option<String>,
    /// SHA-1, hex, as the portal shows it.
    thumbprint: Option<String>,
    enabled: bool,
    /// RFC 3339.
    expires: Option<String>,
    not_before: Option<String>,
    updated: Option<String>,
    /// Whether Key Vault renews it before it expires.
    autorenew: bool,
    tags: BTreeMap<String, String>,
}

impl CertificateRow {
    /// The row for `entry`, a bundle or (when its bundle could not be read)
    /// a listing's entry, in `vault`.
    pub(crate) fn new(vault: &str, name: &str, entry: &Value) -> Self {
        let attributes = &entry["attributes"];
        let policy = &entry["policy"];
        let sans = &policy["x509_props"]["sans"];
        Self {
            id: format!("{vault}/{name}"),
            vault: vault.to_owned(),
            name: name.to_owned(),
            subject: text(&policy["x509_props"]["subject"]),
            sans: [
                strings(&sans["dns_names"]),
                strings(&sans["emails"]),
                strings(&sans["upns"]),
            ]
            .concat(),
            issuer: text(&policy["issuer"]["name"]),
            thumbprint: text(&entry["x5t"]).and_then(|x5t| hex(&x5t)),
            enabled: enabled(attributes),
            expires: from_unix(&attributes["exp"]),
            not_before: from_unix(&attributes["nbf"]),
            updated: from_unix(&attributes["updated"]),
            autorenew: list(&policy["lifetime_actions"])
                .iter()
                .any(|held| held["action"]["action_type"].as_str() == Some("AutoRenew")),
            tags: tags(entry),
        }
    }
}

/// The certificate's name in an id `…/certificates/NAME[/VERSION]`.
pub(crate) fn name_in(entry: &Value) -> Option<String> {
    let id = text(&entry["id"])?;
    let (_, rest) = id.split_once("/certificates/")?;
    rest.split('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Upper-case hex of base64url `encoded` (`x5t`), or `None` when it is not
/// base64url.
fn hex(encoded: &str) -> Option<String> {
    let mut bits = 0u32;
    let mut held = 0;
    let mut out = String::new();
    for byte in encoded.trim_end_matches('=').bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push_str(&format!("{:02X}", (bits >> held) & 0xFF));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::hex;

    #[test]
    fn a_thumbprint_is_its_x5t_in_hex() {
        // SHA-1 of nothing, da39a3ee…, as base64url.
        assert_eq!(
            hex("2jmj7l5rSw0yVb_vlWAYkK_YBwk").as_deref(),
            Some("DA39A3EE5E6B4B0D3255BFEF95601890AFD80709")
        );
        assert_eq!(hex("not base64!"), None);
    }
}
