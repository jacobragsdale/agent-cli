//! What leaves the service gets bounded or scrubbed here, without trusting
//! its schema: documents can carry a 1,536-float vector per field, and
//! definitions can carry keys the service did not redact.

use serde_json::Value;

/// What a sentinel-aware PUT keeps as stored.
pub(crate) const UNCHANGED: &str = "<unchanged>";

/// How much of a document prints.
#[derive(Clone, Copy)]
pub(crate) struct Bounds {
    /// Strings are cut at 200 characters and lists at 5 items.
    pub cut: bool,
    /// Numeric lists longer than 16 print whole rather than as `"[N floats]"`.
    pub vectors: bool,
}

/// `value` within `bounds`; the flag says whether anything was cut.
pub(crate) fn bound(value: Value, bounds: Bounds, cut: &mut bool) -> Value {
    match value {
        Value::Array(items)
            if !bounds.vectors && items.len() > 16 && items.iter().all(Value::is_number) =>
        {
            Value::String(format!("[{} floats]", items.len()))
        }
        Value::Array(mut items) => {
            if bounds.cut && items.len() > 5 {
                items.truncate(5);
                *cut = true;
            }
            Value::Array(items.into_iter().map(|v| bound(v, bounds, cut)).collect())
        }
        Value::String(text) if bounds.cut && text.chars().count() > 200 => {
            *cut = true;
            Value::String(format!("{}…", text.chars().take(200).collect::<String>()))
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, bound(value, bounds, cut)))
                .collect(),
        ),
        other => other,
    }
}

/// Patterns that mark a credential inside any string, matched without regard
/// to case; the value after one is masked up to `;`, `&` or the end.
const PATTERNS: [&str; 5] = [
    "accountkey=",
    "sharedaccesssignature=",
    "sig=",
    "password=",
    "applicationsecret=",
];

/// A definition with every place a secret can sit replaced by
/// `"<unchanged>"`, which a PUT reads as "keep what is stored". The service
/// redacts unevenly (`null`, `"<redacted>"`, or not at all), so this scrubs
/// regardless. Booleans are never touched (`key: true` marks the key field),
/// and `null` stays `null`, so "not stored" still reads as such.
pub(crate) fn scrub(value: &mut Value) {
    scrub_in(value, "", false);
}

fn scrub_in(value: &mut Value, parent: &str, keyed: bool) {
    match value {
        Value::Object(map) => {
            // A `key` is a secret only in an AML vectorizer or skill, or a
            // `…ByKey` account: elsewhere it is the key field's flag.
            let by_key = parent.eq_ignore_ascii_case("amlParameters")
                || map
                    .get("@odata.type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind.ends_with("ByKey") || kind.contains("AmlSkill"));
            for (name, held) in map.iter_mut() {
                let lower = name.to_ascii_lowercase();
                let secret = lower == "apikey"
                    || lower == "applicationsecret"
                    || lower.contains("connectionstring")
                    || (lower == "key" && by_key);
                if secret && held.is_string() {
                    *held = Value::String(UNCHANGED.to_owned());
                } else if lower == "httpheaders" || lower == "headers" {
                    scrub_in(held, name, true);
                } else if lower == "uri" {
                    if let Value::String(uri) = held {
                        *uri = masked(uri, "code=");
                    }
                    scrub_in(held, name, false);
                } else {
                    scrub_in(held, name, false);
                }
            }
            if keyed {
                for held in map.values_mut().filter(|held| held.is_string()) {
                    *held = Value::String(UNCHANGED.to_owned());
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                scrub_in(item, parent, false);
            }
        }
        Value::String(text) => {
            for pattern in PATTERNS {
                *text = masked(text, pattern);
            }
        }
        _ => {}
    }
}

/// `text` with the value after each `pattern` (any case) masked.
fn masked(text: &str, pattern: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (found, _) in lower.match_indices(pattern) {
        // `sig=` inside `xsig=` is another parameter.
        let boundary = found == 0 || !lower.as_bytes()[found - 1].is_ascii_alphanumeric();
        if found < at || !boundary {
            continue;
        }
        let start = found + pattern.len();
        let end = text[start..]
            .find([';', '&', '"', ' '])
            .map_or(text.len(), |offset| start + offset);
        if text[start..end] == *UNCHANGED {
            continue;
        }
        out.push_str(&text[at..start]);
        out.push_str(UNCHANGED);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_document_is_bounded_without_its_schema() {
        let vector: Vec<f64> = (0..1536).map(|n| f64::from(n) / 1000.0).collect();
        let doc = json!({
            "id": "88121", "summary_vector": vector, "small": [1, 2, 3],
            "summary": "x".repeat(250), "tags": ["a", "b", "c", "d", "e", "f"],
            "nested": {"embedding": vec![0.5; 20]},
        });
        let mut cut = false;
        let listed = bound(
            doc.clone(),
            Bounds {
                cut: true,
                vectors: false,
            },
            &mut cut,
        );
        assert!(cut);
        assert_eq!(listed["summary_vector"], "[1536 floats]");
        assert_eq!(listed["nested"]["embedding"], "[20 floats]");
        assert_eq!(listed["small"], json!([1, 2, 3]));
        assert_eq!(listed["tags"], json!(["a", "b", "c", "d", "e"]));
        assert_eq!(listed["summary"].as_str().unwrap().chars().count(), 201);

        let mut cut = false;
        let whole = bound(
            doc.clone(),
            Bounds {
                cut: false,
                vectors: false,
            },
            &mut cut,
        );
        assert!(!cut);
        assert_eq!(whole["summary"], doc["summary"]);
        assert_eq!(whole["summary_vector"], "[1536 floats]");
        let all = bound(
            doc.clone(),
            Bounds {
                cut: false,
                vectors: true,
            },
            &mut cut,
        );
        assert_eq!(all, doc);
    }

    #[test]
    fn every_place_a_secret_sits_is_scrubbed_and_flags_and_nulls_are_not() {
        let mut definition = json!({
            "fields": [{"name": "id", "key": true}],
            "vectorSearch": {"vectorizers": [
                {"name": "aoai", "azureOpenAIParameters": {"resourceUri": "https://aoai-contoso.openai.azure.com", "apiKey": "fixture-aoai-key"}},
                {"name": "web", "customWebApiParameters": {"uri": "https://fn-contoso.azurewebsites.net/api/embed?code=fixture-code&x=1", "httpHeaders": {"x-functions-key": "fixture-header"}}},
                {"name": "aml", "amlParameters": {"uri": "https://aml.contoso.example/score", "key": "fixture-aml-key"}},
            ]},
            "encryptionKey": {"keyVaultUri": "https://kv-contoso.vault.azure.net", "accessCredentials": {"applicationId": "app", "applicationSecret": "fixture-secret"}},
            "credentials": {"connectionString": null},
            "note": "DefaultEndpointsProtocol=https;AccountName=contoso;AccountKey=fixture-account-key;EndpointSuffix=core.windows.net",
            "sas": "https://contoso.blob.core.windows.net/c?sv=1&sig=fixture-sig",
            "design": "design=blue",
        });
        scrub(&mut definition);
        let text = definition.to_string();
        assert!(!text.contains("fixture"), "{text}");
        assert_eq!(definition["fields"][0]["key"], true);
        assert_eq!(definition["credentials"]["connectionString"], Value::Null);
        assert_eq!(
            definition["vectorSearch"]["vectorizers"][1]["customWebApiParameters"]["uri"],
            "https://fn-contoso.azurewebsites.net/api/embed?code=<unchanged>&x=1"
        );
        assert_eq!(
            definition["note"],
            "DefaultEndpointsProtocol=https;AccountName=contoso;AccountKey=<unchanged>;EndpointSuffix=core.windows.net"
        );
        assert_eq!(
            definition["design"], "design=blue",
            "sig= only after a separator"
        );
        assert_eq!(
            definition["vectorSearch"]["vectorizers"][0]["azureOpenAIParameters"]["resourceUri"],
            "https://aoai-contoso.openai.azure.com"
        );
    }
}
