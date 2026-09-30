//! Credentials, and the redaction every error and dry-run plan passes through.
//!
//! Anything this program prints to stderr lands in an agent's transcript, and
//! transcripts get pasted, logged and shared. So every error line and every
//! `--dry-run` plan is masked on its way out: the values of every [`Secret`]
//! made in this process, `Bearer`/`Basic` credentials, JWTs, and the value of
//! anything keyed like a password or token.

use std::fmt;
use std::ops::Range;
use std::sync::{Mutex, PoisonError};

use serde_json::Value;

use crate::http::percent_encode;

const MASK: &str = "***";
/// Shorter values would mask ordinary words wherever they appear.
const MIN_KNOWN: usize = 6;
/// Every secret value made in this process, so redaction can find it however
/// it was spliced into a message.
static KNOWN: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Keys whose value is masked in text: `key=value`, `key: value`, `"key":"value"`.
const TEXT_KEYS: [&str; 11] = [
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
    "sig",
    "authorization",
    "cookie",
];

/// A credential or a secret's value.
///
/// Neither `Debug` nor `Display` prints it and it has no `Serialize`, so it
/// cannot reach a log line, an error, the cache or stdout by accident.
/// [`Secret::expose`] is the one way to read it; a grep for it is the audit.
#[derive(Clone, Eq, PartialEq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value and remembers it, so [`redact`] masks it wherever it
    /// turns up later, raw or percent-encoded.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        if value.len() >= MIN_KNOWN {
            let mut known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
            if !known.contains(&value) {
                known.push(value.clone());
            }
        }
        Self(value)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

/// `text` with every credential in it replaced by `***`.
#[must_use]
pub fn redact(text: &str) -> String {
    let mut text = text.to_owned();
    let known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner).clone();
    for value in known {
        let mut encoded = String::new();
        percent_encode(&value, &mut encoded);
        for form in [value, encoded] {
            if text.contains(&form) {
                text = text.replace(&form, MASK);
            }
        }
    }
    let mut spans = secret_spans(&text);
    if spans.is_empty() {
        return text;
    }
    spans.sort_by_key(|span| span.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for span in spans {
        if span.start < at {
            // Overlaps the previous mask, which already covered its start.
            at = at.max(span.end);
            continue;
        }
        out.push_str(&text[at..span.start]);
        out.push_str(MASK);
        at = span.end;
    }
    out.push_str(&text[at..]);
    out
}

/// A JSON value with every string redacted and every sensitive key's value
/// masked outright. What `--dry-run` prints goes through this.
#[must_use]
pub fn redact_value(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let value = match value {
                        Value::String(_) | Value::Number(_) if sensitive_key(&key) => {
                            Value::String(MASK.to_owned())
                        }
                        other => redact_value(other),
                    };
                    (key, value)
                })
                .collect(),
        ),
        other => other,
    }
}

/// True for a header or field name whose value is a credential.
#[must_use]
pub fn sensitive_key(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "pwd"
        || name == "sig"
        || [
            "password",
            "passwd",
            "secret",
            "token",
            "authorization",
            "cookie",
            "apikey",
            "api_key",
            "api-key",
        ]
        .iter()
        .any(|word| name.contains(word))
}

/// Byte ranges of credential values in `text`. Every range starts and ends on
/// an ASCII byte or the end, so slicing at them is always on a char boundary.
fn secret_spans(text: &str) -> Vec<Range<usize>> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut spans = Vec::new();
    for scheme in ["bearer ", "basic "] {
        for (at, _) in lower.match_indices(scheme) {
            let start = skip(bytes, at + scheme.len(), |byte| byte == b' ');
            let end = skip(bytes, start, is_token_byte);
            if end > start && &text[start..end] != MASK {
                spans.push(start..end);
            }
        }
    }
    for key in TEXT_KEYS {
        for (at, _) in lower.match_indices(key) {
            if let Some(span) = value_after(text, bytes, at + key.len()) {
                spans.push(span);
            }
        }
    }
    let raw = text.as_bytes();
    for (at, _) in text.match_indices("eyJ") {
        let end = skip(raw, at, |byte| is_token_byte(byte) && byte != b'/');
        let dots = raw[at..end].iter().filter(|&&byte| byte == b'.').count();
        if dots >= 2 && end - at >= 30 {
            spans.push(at..end);
        }
    }
    spans
}

/// The value of `key=value`, `key: value` or `"key": "value"` whose key ends
/// at `at`, when a separator follows it.
fn value_after(text: &str, bytes: &[u8], at: usize) -> Option<Range<usize>> {
    let mut index = at;
    if matches!(bytes.get(index), Some(b'"' | b'\'')) {
        index += 1;
    }
    index = skip(bytes, index, |byte| byte == b' ');
    if !matches!(bytes.get(index), Some(b'=' | b':')) {
        return None;
    }
    index = skip(bytes, index + 1, |byte| byte == b' ');
    let (start, end) = match bytes.get(index) {
        Some(&quote @ (b'"' | b'\'')) => {
            let start = index + 1;
            (start, skip(bytes, start, |byte| byte != quote))
        }
        _ => (
            index,
            skip(bytes, index, |byte| {
                !byte.is_ascii_whitespace() && !b"\"';&,})]".contains(&byte)
            }),
        ),
    };
    (end > start && &text[start..end] != MASK).then_some(start..end)
}

fn skip(bytes: &[u8], mut index: usize, keep: impl Fn(u8) -> bool) -> usize {
    while index < bytes.len() && keep(bytes[index]) {
        index += 1;
    }
    index
}

/// The characters of a bearer token, a base64 credential or a JWT.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_secret_never_prints_and_is_masked_wherever_it_turns_up() {
        let secret = Secret::new("s3cr3t value/+");
        assert_eq!(format!("{secret} {secret:?}"), "[redacted] [redacted]");
        assert_eq!(secret.expose(), "s3cr3t value/+");
        let said = redact("got s3cr3t value/+ and s3cr3t+value%2F%2B in a url");
        assert_eq!(said, "got *** and *** in a url");
    }

    #[test]
    fn schemes_pairs_and_jwts_are_masked_and_ordinary_text_is_not() {
        let cases = [
            ("Authorization: Bearer abc123.def", "Authorization: *** ***"),
            ("header basic dXNlcjpwYXNz= sent", "header basic *** sent"),
            (
                "Server=db;User Id=app;Password=hunter2;Encrypt=true",
                "Server=db;User Id=app;Password=***;Encrypt=true",
            ),
            (
                r#"{"password": "a b c", "user": "x"}"#,
                r#"{"password": "***", "user": "x"}"#,
            ),
            (
                "https://x/blob?sv=1&sig=AbC%2F&se=2",
                "https://x/blob?sv=1&sig=***&se=2",
            ),
            ("access_token=xyz; path=/", "access_token=***; path=/"),
            (
                "token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJlLXZhbHVl done",
                "token *** done",
            ),
            (
                "list secrets in kv secret get",
                "list secrets in kv secret get",
            ),
            ("assignee=me design: flat", "assignee=me design: flat"),
            ("password_env = \"DB_PASS\"", "password_env = \"DB_PASS\""),
        ];
        for (text, want) in cases {
            assert_eq!(redact(text), want, "{text}");
        }
        assert_eq!(
            redact("héllo Bearer abc ünïcode"),
            "héllo Bearer *** ünïcode"
        );
        assert_eq!(redact("héllo Bearer ünïcode"), "héllo Bearer ünïcode");
    }

    #[test]
    fn a_plan_masks_sensitive_keys_and_redacts_every_string() {
        let plan = json!({
            "headers": {"Authorization": "Bearer x", "Accept": "application/json"},
            "body": {"db_password": "p", "note": "Bearer abcdef", "count": 3},
            "run": ["sqlcmd", "-P", "pwd=zzz"],
        });
        assert_eq!(
            redact_value(plan),
            json!({
                "headers": {"Authorization": "***", "Accept": "application/json"},
                "body": {"db_password": "***", "note": "Bearer ***", "count": 3},
                "run": ["sqlcmd", "-P", "pwd=***"],
            })
        );
        assert!(sensitive_key("X-Api-Key") && sensitive_key("SIG") && !sensitive_key("assignee"));
    }
}
