//! What the key verbs share: a key's name as it prints and as it is typed
//! back, reading one key whole or in part, and turning stored bytes into
//! something an agent can read.
//!
//! Values decode by what their bytes are, never by configuration: a .NET
//! `IDistributedCache` hash, gzip or zlib (decompressed), JSON, text, a
//! Python pickle (named, never loaded), zstd, lz4 or xz (named), else
//! base64.

pub mod diff;
pub mod get;
pub mod list;

use std::io::Read;

use agent_cli_core::{Failure, base64, now, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::session::{Reply, Session, cmd};

/// The most of a string the door fetches and a decompressor writes.
pub const READ: usize = 1 << 20;

/// The most of a value `key get` prints; `--output FILE` saves all of it.
pub const SHOW: usize = 8 << 10;

/// The most of one member of a collection `key get` prints.
pub const MEMBER: usize = 512;

/// A key as it prints: itself when it is UTF-8 text, else with each byte
/// that is not (and each control character) as `\xNN`, and `\` as `\\`,
/// which [`unescape`] reads back.
pub fn escape(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes)
        && !text.bytes().any(|byte| byte.is_ascii_control())
    {
        return text.to_owned();
    }
    let mut out = String::new();
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                c if c.is_ascii_control() => out.push_str(&format!("\\x{:02x}", c as u8)),
                c => out.push(c),
            }
        }
        for byte in chunk.invalid() {
            out.push_str(&format!("\\x{byte:02x}"));
        }
    }
    out
}

/// A key as typed: `\xNN` escapes are bytes when the text holds one.
// ponytail: a UTF-8 key that itself holds the text `\x` cannot be typed;
// a --hex flag if one turns up.
pub fn unescape(text: &str) -> Vec<u8> {
    if !text.contains("\\x") {
        return text.as_bytes().to_vec();
    }
    let mut out = Vec::new();
    let mut rest = text.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        match (byte, tail) {
            (b'\\', [b'\\', after @ ..]) => {
                out.push(b'\\');
                rest = after;
            }
            (b'\\', [b'x', hi, lo, after @ ..]) if hex(*hi).is_some() && hex(*lo).is_some() => {
                out.push(hex(*hi).unwrap_or_default() << 4 | hex(*lo).unwrap_or_default());
                rest = after;
            }
            _ => {
                out.push(byte);
                rest = tail;
            }
        }
    }
    out
}

fn hex(digit: u8) -> Option<u8> {
    (digit as char).to_digit(16).map(|value| value as u8)
}

/// What a string held, decoded.
#[derive(Debug, Default, PartialEq)]
pub struct Decoded {
    pub value: Value,
    /// json, text, pickle or base64.
    pub format: &'static str,
    pub compression: Option<&'static str>,
    pub truncated: bool,
}

/// `raw` is the whole value when `whole`, else its first bytes. `show` is
/// the most of it to keep.
pub fn decode(raw: &[u8], whole: bool, show: usize) -> Decoded {
    let (bytes, compression, complete) = match compression(raw) {
        Some(kind @ ("gzip" | "zlib")) => match inflate(raw, kind, READ) {
            Some((out, done)) => (out, Some(kind), whole && done),
            None => (raw.to_vec(), None, whole),
        },
        Some(kind) => {
            let cut = show * 3 / 4;
            return Decoded {
                value: Value::String(base64(&raw[..raw.len().min(cut)])),
                format: "base64",
                compression: Some(kind),
                truncated: raw.len() > cut || !whole,
            };
        }
        None => (raw.to_vec(), None, whole),
    };
    if is_pickle(&bytes) {
        return Decoded {
            value: Value::Null,
            format: "pickle",
            compression,
            truncated: false,
        };
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => Some(text),
        // Cut inside a character: the rest of it is past what was read.
        Err(error) if !complete && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    };
    let Some(text) = text else {
        let cut = show * 3 / 4;
        return Decoded {
            value: Value::String(base64(&bytes[..bytes.len().min(cut)])),
            format: "base64",
            compression,
            truncated: bytes.len() > cut || !complete,
        };
    };
    let json = complete
        .then(|| serde_json::from_str::<Value>(text).ok())
        .flatten()
        .filter(|parsed| parsed.is_object() || parsed.is_array());
    let format = if json.is_some() { "json" } else { "text" };
    if let Some(parsed) = json.filter(|_| text.len() <= show) {
        return Decoded {
            value: parsed,
            format,
            compression,
            truncated: false,
        };
    }
    let kept = cut(text, show);
    Decoded {
        truncated: kept.len() < text.len() || !complete,
        value: Value::String(kept.to_owned()),
        format,
        compression,
    }
}

/// One member of a collection, as JSON when it is, else text or base64,
/// cut at `show` bytes with how long it was.
pub fn member(raw: &[u8], show: usize) -> Value {
    let decoded = decode(raw, true, show);
    match decoded.format {
        "pickle" => Value::String(format!("[pickle, {} bytes]", raw.len())),
        _ if decoded.truncated => Value::String(format!(
            "{}… [{} bytes]",
            decoded.value.as_str().unwrap_or_default(),
            raw.len()
        )),
        _ => decoded.value,
    }
}

fn cut(text: &str, at: usize) -> &str {
    if text.len() <= at {
        return text;
    }
    let mut end = at;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// What compressed `bytes` by their magic number, if anything did.
fn compression(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x1f, 0x8b, ..] => Some("gzip"),
        [0x78, 0x01 | 0x5e | 0x9c | 0xda, ..] => Some("zlib"),
        [0x28, 0xb5, 0x2f, 0xfd, ..] => Some("zstd"),
        [0x04, 0x22, 0x4d, 0x18, ..] => Some("lz4"),
        [0xfd, b'7', b'z', b'X', b'Z', 0x00, ..] => Some("xz"),
        _ => None,
    }
}

/// Up to `cap` bytes of `raw` decompressed, and whether that is all of
/// it; `None` when it does not decompress at all (a text that begins
/// `x^`). A value cut short gives what it held up to the cut.
fn inflate(raw: &[u8], kind: &str, cap: usize) -> Option<(Vec<u8>, bool)> {
    let reader: Box<dyn Read> = match kind {
        "gzip" => Box::new(flate2::read::MultiGzDecoder::new(raw)),
        _ => Box::new(flate2::read::ZlibDecoder::new(raw)),
    };
    let mut out = Vec::new();
    match reader.take(cap as u64 + 1).read_to_end(&mut out) {
        Ok(_) if out.len() > cap => {
            out.truncate(cap);
            Some((out, false))
        }
        Ok(_) => Some((out, true)),
        Err(_) if !out.is_empty() => Some((out, false)),
        Err(_) => None,
    }
}

/// A whole gzip or zlib value decompressed, for `--output FILE`: at most
/// 256 MB, so a small value that expands without end cannot fill the disk.
pub fn inflate_all(raw: &[u8], kind: &str) -> Vec<u8> {
    inflate(raw, kind, 256 << 20).map_or_else(|| raw.to_vec(), |(out, _)| out)
}

/// Python's pickle protocol 2 to 5 opens with PROTO and its number.
fn is_pickle(bytes: &[u8]) -> bool {
    matches!(bytes, [0x80, 2..=5, ..])
}

/// .NET ticks (100 ns since 0001-01-01) of an `IDistributedCache` entry's
/// `absexp`, as a time; -1 is unset.
fn ticks_at(ticks: i64) -> Option<String> {
    const UNIX: i64 = 621_355_968_000_000_000;
    (ticks > UNIX)
        .then(|| time::OffsetDateTime::from_unix_timestamp((ticks - UNIX) / 10_000_000).ok())
        .flatten()
        .map(utc_time)
}

/// One key, read for `key get` or `key diff`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Key {
    /// What key get takes with --instance.
    pub id: String,
    /// string, hash, list, set, zset, stream, or a module's type.
    #[serde(rename = "type")]
    pub kind: String,
    /// How Redis stores it: listpack, hashtable, int, embstr, raw …
    pub encoding: Option<String>,
    /// Bytes of a string, members of anything else.
    pub length: Option<i64>,
    /// Memory it takes, in bytes, sampled for big collections.
    pub size_bytes: Option<i64>,
    /// Absent: it never expires. Redis keeps no creation time.
    pub expires_at: Option<String>,
    pub ttl_s: Option<i64>,
    /// When it was last read or written, before this read; an earlier key
    /// get counts, the metadata agent-cli asks for does not.
    pub last_access: Option<String>,
    /// Under an LFU maxmemory_policy, which tracks no last access: a
    /// logarithmic counter, 0 to 255.
    pub access_frequency: Option<i64>,
    /// IDistributedCache: the value is a .NET distributed cache entry, its
    /// data decoded as `value`.
    pub envelope: Option<String>,
    /// The entry's own absolute expiry.
    pub absolute_expires_at: Option<String>,
    /// The entry's sliding window: each read pushes expires_at this far out.
    pub sliding_s: Option<i64>,
    /// gzip or zlib (decompressed); zstd, lz4 or xz (left as base64).
    pub compression: Option<String>,
    /// json, text, base64 or pickle (no value: pickles are never loaded).
    pub format: Option<String>,
    /// Only part of the value is here.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub value: Value,
}

/// How much of a key to read.
#[derive(Clone, Copy)]
pub struct Reach {
    /// Bytes of a string to keep.
    pub show: usize,
    /// Bytes of each member of a collection to keep.
    pub member: usize,
    /// Members of a collection.
    pub members: usize,
}

/// Reads `key`: what it is, when it expires and was last used, and its
/// value as far as `reach` goes. A missing key is exit 4 with the list
/// that finds its neighbours.
pub fn read(session: &mut Session, key: &[u8], reach: Reach) -> Result<Key> {
    let meta = session.pipeline(&[
        cmd!("TYPE", key),
        cmd!("PTTL", key),
        cmd!("MEMORY", "USAGE", key),
        cmd!("OBJECT", "ENCODING", key),
        cmd!("OBJECT", "IDLETIME", key),
        cmd!("OBJECT", "FREQ", key),
    ])?;
    let kind = meta[0].text().unwrap_or_default();
    let id = escape(key);
    if kind == "none" || kind.is_empty() {
        if let Reply::Error(message) = &meta[0] {
            return Err(session.refused(message));
        }
        return Err(
            Failure::not_found(format!("{}: no key {id}", session.instance))
                .hint(format!(
                    "agent-cli redis key list {} --instance {}",
                    neighbours(&id),
                    session.instance
                ))
                .into(),
        );
    }
    let moment = now();
    let ttl = meta[1].int().filter(|ms| *ms >= 0);
    let mut row = Key {
        id,
        encoding: meta[3].text(),
        length: None,
        size_bytes: meta[2].int(),
        expires_at: ttl.map(|ms| utc_time(moment + time::Duration::milliseconds(ms))),
        ttl_s: ttl.map(|ms| (ms + 999) / 1000),
        last_access: meta[4]
            .int()
            .map(|idle| utc_time(moment - time::Duration::seconds(idle))),
        access_frequency: meta[5].int(),
        envelope: None,
        absolute_expires_at: None,
        sliding_s: None,
        compression: None,
        format: None,
        truncated: false,
        value: Value::Null,
        kind,
    };
    match row.kind.as_str() {
        "string" => {
            let got = session.pipeline(&[
                cmd!("STRLEN", key),
                cmd!("GETRANGE", key, "0", (READ - 1).to_string()),
            ])?;
            let length = got[0].int().unwrap_or_default();
            let bytes = got[1].bytes().unwrap_or_default();
            row.length = Some(length);
            row.set(decode(bytes, length as usize <= bytes.len(), reach.show));
        }
        "hash" => {
            row.length = session.call(cmd!("HLEN", key))?.int();
            let pairs = scan(session, "HSCAN", key, reach.members, 2)?;
            if row.length == Some(3) && dotnet(&mut row, &pairs, reach) {
                return Ok(row);
            }
            let mut fields = Map::new();
            for pair in &pairs {
                fields.insert(escape(&pair[0]), member(&pair[1], reach.member));
            }
            row.truncated = (fields.len() as i64) < row.length.unwrap_or_default();
            row.value = Value::Object(fields);
        }
        "list" => {
            let got = session.pipeline(&[
                cmd!("LLEN", key),
                cmd!("LRANGE", key, "0", (reach.members.max(1) - 1).to_string()),
            ])?;
            row.collection(&got, |item| {
                member(item.bytes().unwrap_or_default(), reach.member)
            });
        }
        "set" => {
            row.length = session.call(cmd!("SCARD", key))?.int();
            let mut members: Vec<Value> = scan(session, "SSCAN", key, reach.members, 1)?
                .iter()
                .map(|one| member(&one[0], reach.member))
                .collect();
            members.sort_by_key(Value::to_string);
            row.truncated = (members.len() as i64) < row.length.unwrap_or_default();
            row.value = Value::Array(members);
        }
        "zset" => {
            let got = session.pipeline(&[
                cmd!("ZCARD", key),
                cmd!(
                    "ZRANGE",
                    key,
                    "0",
                    (reach.members.max(1) - 1).to_string(),
                    "WITHSCORES"
                ),
            ])?;
            row.length = got[0].int();
            let members: Vec<Value> = got[1]
                .items()
                .chunks(2)
                .map(|pair| {
                    json!({
                        "member": member(pair[0].bytes().unwrap_or_default(), reach.member),
                        "score": pair.get(1).and_then(Reply::text).and_then(|score| score.parse::<f64>().ok()),
                    })
                })
                .collect();
            row.truncated = (members.len() as i64) < row.length.unwrap_or_default();
            row.value = Value::Array(members);
        }
        "stream" => {
            let got = session.pipeline(&[
                cmd!("XLEN", key),
                cmd!(
                    "XREVRANGE",
                    key,
                    "+",
                    "-",
                    "COUNT",
                    reach.members.to_string()
                ),
            ])?;
            row.collection(&got, |entry| {
                let items = entry.items();
                let mut fields = Map::new();
                for pair in items.get(1).map(Reply::items).unwrap_or_default().chunks(2) {
                    fields.insert(
                        pair[0].text().unwrap_or_default(),
                        pair.get(1).map_or(Value::Null, |value| {
                            member(value.bytes().unwrap_or_default(), reach.member)
                        }),
                    );
                }
                json!({"id": items.first().and_then(Reply::text), "fields": fields})
            });
        }
        _ => {}
    }
    Ok(row)
}

impl Key {
    fn set(&mut self, decoded: Decoded) {
        self.format = Some(decoded.format.to_owned());
        self.compression = decoded.compression.map(str::to_owned);
        self.truncated = decoded.truncated;
        self.value = decoded.value;
    }

    /// A length and a page of members, from a `*LEN` and a range read.
    fn collection(&mut self, got: &[Reply], each: impl Fn(&Reply) -> Value) {
        self.length = got[0].int();
        let members: Vec<Value> = got[1].items().iter().map(each).collect();
        self.truncated = (members.len() as i64) < self.length.unwrap_or_default();
        self.value = Value::Array(members);
    }
}

/// A hash of exactly `absexp`, `sldexp` and `data` is how .NET's
/// `IDistributedCache` (Microsoft.Extensions.Caching.StackExchangeRedis)
/// stores an entry.
fn dotnet(row: &mut Key, pairs: &[Vec<Vec<u8>>], reach: Reach) -> bool {
    let field = |name: &str| {
        pairs
            .iter()
            .find(|pair| pair[0] == name.as_bytes())
            .map(|pair| pair[1].as_slice())
    };
    let (Some(absolute), Some(sliding), Some(data)) =
        (field("absexp"), field("sldexp"), field("data"))
    else {
        return false;
    };
    let ticks = |raw: &[u8]| std::str::from_utf8(raw).ok()?.parse::<i64>().ok();
    row.envelope = Some("IDistributedCache".to_owned());
    row.absolute_expires_at = ticks(absolute).and_then(ticks_at);
    row.sliding_s = ticks(sliding)
        .filter(|ticks| *ticks > 0)
        .map(|ticks| ticks / 10_000_000);
    row.length = Some(data.len() as i64);
    row.set(decode(data, true, reach.show));
    true
}

/// `HSCAN` or `SSCAN` until `want` members or the end; each member is
/// `width` replies (a field and its value, or one member).
fn scan(
    session: &mut Session,
    verb: &str,
    key: &[u8],
    want: usize,
    width: usize,
) -> Result<Vec<Vec<Vec<u8>>>> {
    let mut found = Vec::new();
    let mut cursor = "0".to_owned();
    loop {
        let page = session.call(cmd!(
            verb,
            key,
            &cursor,
            "COUNT",
            want.clamp(10, 1000).to_string()
        ))?;
        let items = page.items();
        cursor = items
            .first()
            .and_then(Reply::text)
            .unwrap_or_else(|| "0".to_owned());
        for chunk in items
            .get(1)
            .map(Reply::items)
            .unwrap_or_default()
            .chunks(width)
        {
            found.push(
                chunk
                    .iter()
                    .map(|one| one.bytes().unwrap_or_default().to_vec())
                    .collect(),
            );
        }
        if cursor == "0" || found.len() >= want {
            found.truncate(want);
            return Ok(found);
        }
    }
}

/// A pattern for the keys beside `id`: its prefix to the last `:`.
fn neighbours(id: &str) -> String {
    let pattern = match id.rfind([':', '/', '.']) {
        Some(end) if end > 0 => format!("{}*", &id[..=end]),
        _ => format!("{}*", id.chars().take(3).collect::<String>()),
    };
    if pattern
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\'')
    {
        "PATTERN".to_owned()
    } else {
        pattern
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn zlib(bytes: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn values_decode_by_what_their_bytes_are() {
        let json = br#"{"order":4417,"status":"shipped"}"#;
        assert_eq!(
            decode(json, true, SHOW),
            Decoded {
                value: json!({"order": 4417, "status": "shipped"}),
                format: "json",
                compression: None,
                truncated: false
            }
        );
        assert_eq!(
            decode(&gzip(json), true, SHOW).value,
            json!({"order": 4417, "status": "shipped"})
        );
        assert_eq!(decode(&gzip(json), true, SHOW).compression, Some("gzip"));
        assert_eq!(decode(&zlib(json), true, SHOW).compression, Some("zlib"));
        assert_eq!(
            decode(b"x^ a text that is no zlib", true, SHOW).format,
            "text"
        );
        assert_eq!(
            decode(b"42", true, SHOW).value,
            json!("42"),
            "a scalar stays text"
        );
        let pickle = decode(
            b"\x80\x04\x95\x10\x00\x00\x00\x00\x00\x00\x00}\x94.",
            true,
            SHOW,
        );
        assert_eq!((pickle.format, pickle.value), ("pickle", Value::Null));
        let zstd = decode(b"\x28\xb5\x2f\xfd\x00\x01", true, SHOW);
        assert_eq!(
            (zstd.format, zstd.compression, zstd.value),
            ("base64", Some("zstd"), json!("KLUv/QAB"))
        );
        assert_eq!(decode(&[0xff, 0xfe, 0x00], true, SHOW).value, json!("//4A"));
    }

    #[test]
    fn a_long_value_keeps_its_first_bytes_and_says_so() {
        let long = format!("[{}0]", "0,".repeat(10_000));
        let cut = decode(long.as_bytes(), true, SHOW);
        assert_eq!((cut.format, cut.truncated), ("json", true));
        assert_eq!(cut.value.as_str().unwrap().len(), SHOW);
        let partial = decode("é".repeat(10).as_bytes(), false, SHOW);
        assert!(partial.truncated && partial.format == "text");
        let mut gz = gzip("é".repeat(100).as_bytes());
        gz.truncate(gz.len() - 8);
        assert_eq!(
            decode(&gz, true, SHOW).compression,
            Some("gzip"),
            "a cut gzip gives what it held"
        );
        assert_eq!(
            member(&[b'a'; 600], MEMBER).as_str().unwrap().len(),
            MEMBER + "… [600 bytes]".len()
        );
    }

    #[test]
    fn keys_that_are_not_utf8_print_escaped_and_read_back() {
        assert_eq!(escape(b"session:42"), "session:42");
        let odd = b"bin:\xff\x00\\x";
        assert_eq!(escape(odd), "bin:\\xff\\x00\\\\x");
        assert_eq!(unescape(&escape(odd)), odd);
        assert_eq!(unescape("plain\\path"), b"plain\\path");
    }

    #[test]
    fn dotnet_ticks_are_times_and_a_missing_key_points_at_its_neighbours() {
        assert_eq!(
            ticks_at(639_000_000_000_000_000).as_deref(),
            Some("2025-11-29T08:00:00Z")
        );
        assert_eq!(ticks_at(-1), None);
        assert_eq!(neighbours("session:7f3a"), "session:*");
        assert_eq!(neighbours(":1:report:7"), ":1:report:*");
        assert_eq!(neighbours("orders"), "ord*");
        assert_eq!(neighbours("a b:c"), "PATTERN");
    }
}
