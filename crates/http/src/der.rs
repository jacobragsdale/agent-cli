//! The few fields of an X.509 certificate a probe prints, read from its DER
//! (rustls hands certificates over as DER only), and its SHA-1 thumbprint,
//! the form Key Vault prints.
// ponytail: names, validity, SANs, basic constraints, the key and the
// signature algorithm only; no name constraints, policies or OCSP. An X.509
// crate if a command needs those.

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

/// What a certificate says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Certificate {
    pub(crate) subject: String,
    pub(crate) issuer: String,
    pub(crate) serial: String,
    pub(crate) not_before: Option<OffsetDateTime>,
    pub(crate) not_after: Option<OffsetDateTime>,
    pub(crate) dns_names: Vec<String>,
    pub(crate) ip_addresses: Vec<String>,
    pub(crate) is_ca: bool,
    /// `RSA 2048`, `EC P-256`, `Ed25519`.
    pub(crate) key: String,
    pub(crate) signature: String,
    /// SHA-1 of the DER, uppercase hex: Key Vault's thumbprint.
    pub(crate) thumbprint: String,
}

/// One TLV: its tag, its contents, and what follows it.
fn read(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return None;
        }
        let len = rest[..count]
            .iter()
            .fold(0usize, |len, b| (len << 8) | usize::from(*b));
        (len, &rest[count..])
    };
    if rest.len() < len {
        return None;
    }
    Some((tag, &rest[..len], &rest[len..]))
}

/// The items of a SEQUENCE or SET.
fn items(mut input: &[u8]) -> Vec<(u8, &[u8])> {
    let mut found = Vec::new();
    while let Some((tag, body, rest)) = read(input) {
        found.push((tag, body));
        input = rest;
    }
    found
}

pub(crate) fn parse(der: &[u8]) -> Option<Certificate> {
    let (_, certificate, _) = read(der)?;
    let parts = items(certificate);
    let (_, tbs) = *parts.first()?;
    let signature = parts
        .get(1)
        .map(|(_, alg)| algorithm(alg))
        .unwrap_or_default();
    let mut fields = items(tbs).into_iter();
    let mut next = fields.next()?;
    if next.0 == 0xa0 {
        next = fields.next()?;
    }
    let serial = hex(strip_zero(next.1), ":");
    let _signature_alg = fields.next()?;
    let issuer = name(fields.next()?.1);
    let validity = items(fields.next()?.1);
    let subject = name(fields.next()?.1);
    let key = public_key(fields.next()?.1);
    let mut cert = Certificate {
        subject,
        issuer,
        serial,
        not_before: validity.first().and_then(|(tag, body)| when(*tag, body)),
        not_after: validity.get(1).and_then(|(tag, body)| when(*tag, body)),
        key,
        signature,
        thumbprint: hex(&sha1(der), ""),
        ..Certificate::default()
    };
    for (tag, body) in fields {
        if tag != 0xa3 {
            continue;
        }
        let Some((_, list, _)) = read(body) else {
            continue;
        };
        for (_, extension) in items(list) {
            let parts = items(extension);
            let Some((_, oid_bytes)) = parts.first() else {
                continue;
            };
            let Some((_, value)) = parts.iter().rev().find(|(tag, _)| *tag == 0x04) else {
                continue;
            };
            match oid(oid_bytes).as_str() {
                "2.5.29.17" => {
                    if let Some((_, names, _)) = read(value) {
                        for (tag, item) in items(names) {
                            match tag {
                                0x82 => cert
                                    .dns_names
                                    .push(String::from_utf8_lossy(item).into_owned()),
                                0x87 => cert.ip_addresses.push(ip(item)),
                                _ => {}
                            }
                        }
                    }
                }
                "2.5.29.19" => {
                    if let Some((_, constraints, _)) = read(value) {
                        cert.is_ca = items(constraints)
                            .first()
                            .is_some_and(|(tag, b)| *tag == 0x01 && b.first() == Some(&0xff));
                    }
                }
                _ => {}
            }
        }
    }
    Some(cert)
}

fn strip_zero(bytes: &[u8]) -> &[u8] {
    match bytes {
        [0, rest @ ..] if !rest.is_empty() => rest,
        other => other,
    }
}

fn hex(bytes: &[u8], separator: &str) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(separator)
}

fn oid(bytes: &[u8]) -> String {
    let Some((&first, rest)) = bytes.split_first() else {
        return String::new();
    };
    let mut parts = vec![u64::from(first / 40), u64::from(first % 40)];
    let mut value = 0u64;
    for &byte in rest {
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            parts.push(value);
            value = 0;
        }
    }
    parts
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

/// A Name as `CN=api.contoso.example, O=Contoso`.
fn name(body: &[u8]) -> String {
    let mut parts = Vec::new();
    for (_, set) in items(body) {
        for (_, pair) in items(set) {
            let fields = items(pair);
            let (Some((_, kind)), Some((_, value))) = (fields.first(), fields.get(1)) else {
                continue;
            };
            let label = match oid(kind).as_str() {
                "2.5.4.3" => "CN",
                "2.5.4.10" => "O",
                "2.5.4.11" => "OU",
                "2.5.4.6" => "C",
                "2.5.4.7" => "L",
                "2.5.4.8" => "ST",
                "1.2.840.113549.1.9.1" => "E",
                _ => continue,
            };
            parts.push(format!("{label}={}", String::from_utf8_lossy(value)));
        }
    }
    parts.reverse();
    parts.join(", ")
}

fn algorithm(body: &[u8]) -> String {
    let id = items(body).first().map(|(_, b)| oid(b)).unwrap_or_default();
    match id.as_str() {
        "1.2.840.113549.1.1.11" => "sha256WithRSA",
        "1.2.840.113549.1.1.12" => "sha384WithRSA",
        "1.2.840.113549.1.1.13" => "sha512WithRSA",
        "1.2.840.113549.1.1.5" => "sha1WithRSA",
        "1.2.840.113549.1.1.10" => "RSASSA-PSS",
        "1.2.840.10045.4.3.2" => "ecdsa-with-SHA256",
        "1.2.840.10045.4.3.3" => "ecdsa-with-SHA384",
        "1.2.840.10045.4.3.4" => "ecdsa-with-SHA512",
        "1.3.101.112" => "Ed25519",
        _ => return id,
    }
    .to_owned()
}

fn public_key(body: &[u8]) -> String {
    let parts = items(body);
    let Some((_, alg)) = parts.first() else {
        return String::new();
    };
    let alg_parts = items(alg);
    let kind = alg_parts.first().map(|(_, b)| oid(b)).unwrap_or_default();
    match kind.as_str() {
        "1.2.840.113549.1.1.1" => {
            let bits = parts
                .get(1)
                .and_then(|(_, bit_string)| bit_string.get(1..))
                .and_then(|key| read(key))
                .and_then(|(_, sequence, _)| {
                    items(sequence)
                        .first()
                        .map(|(_, modulus)| strip_zero(modulus).len() * 8)
                });
            bits.map_or_else(|| "RSA".to_owned(), |bits| format!("RSA {bits}"))
        }
        "1.2.840.10045.2.1" => {
            let curve = alg_parts.get(1).map(|(_, b)| oid(b)).unwrap_or_default();
            match curve.as_str() {
                "1.2.840.10045.3.1.7" => "EC P-256",
                "1.3.132.0.34" => "EC P-384",
                "1.3.132.0.35" => "EC P-521",
                _ => "EC",
            }
            .to_owned()
        }
        "1.3.101.112" => "Ed25519".to_owned(),
        other => other.to_owned(),
    }
}

fn ip(bytes: &[u8]) -> String {
    match bytes.len() {
        4 => std::net::Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).to_string(),
        16 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(bytes);
            std::net::Ipv6Addr::from(octets).to_string()
        }
        _ => hex(bytes, ""),
    }
}

/// A UTCTime (`YYMMDDHHMMSSZ`) or GeneralizedTime (`YYYYMMDDHHMMSSZ`).
fn when(tag: u8, body: &[u8]) -> Option<OffsetDateTime> {
    let text = std::str::from_utf8(body).ok()?.strip_suffix('Z')?;
    let (year, rest) = match tag {
        0x17 => {
            let short: i32 = text.get(..2)?.parse().ok()?;
            (
                if short >= 50 {
                    1900 + short
                } else {
                    2000 + short
                },
                text.get(2..)?,
            )
        }
        0x18 => (text.get(..4)?.parse().ok()?, text.get(4..)?),
        _ => return None,
    };
    let number = |at: usize| -> Option<u8> { rest.get(at..at + 2)?.parse().ok() };
    let date =
        Date::from_calendar_date(year, Month::try_from(number(0)?).ok()?, number(2)?).ok()?;
    let time = Time::from_hms(number(4)?, number(6)?, number(8).unwrap_or(0)).ok()?;
    Some(PrimitiveDateTime::new(date, time).assume_utc())
}

/// SHA-1, for the thumbprint only: an identifier, not a check of anything.
pub(crate) fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (held, add) in h.iter_mut().zip([a, b, c, d, e]) {
            *held = held.wrapping_add(add);
        }
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_its_known_answers() {
        assert_eq!(
            hex(&sha1(b""), ""),
            "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709"
        );
        assert_eq!(
            hex(&sha1(b"abc"), ""),
            "A9993E364706816ABA3E25717850C26C9CD0D89D"
        );
        assert_eq!(
            hex(
                &sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
                ""
            ),
            "84983E441C3BD26EBAAE4AA1F95129E5E54670F1"
        );
    }

    #[test]
    fn a_certificate_reads_its_names_validity_and_key() {
        let cert = parse(&crate::testing::leaf_der()).unwrap();
        assert_eq!(cert.subject, "CN=api.contoso.test, O=Contoso");
        assert_eq!(cert.issuer, "CN=Contoso Test Root CA, O=Contoso");
        assert_eq!(cert.dns_names, ["localhost", "api.contoso.test"]);
        assert_eq!(cert.ip_addresses, ["127.0.0.1"]);
        assert!(!cert.is_ca);
        assert_eq!(cert.key, "EC P-256");
        assert_eq!(cert.signature, "ecdsa-with-SHA256");
        assert_eq!(cert.not_after.unwrap().year(), 2036);
        assert_eq!(cert.thumbprint, "E4298A2276E4835F9C116E220E8507CA074A2A83");
    }
}
