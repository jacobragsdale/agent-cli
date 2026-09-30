//! `k8s configmap`: keys, and their values.

pub(crate) mod get;
pub(crate) mod list;

use std::collections::BTreeMap;

use serde_json::Value;

use crate::kubectl::decoded_len;

/// A configmap's keys and values, a binary key's value being its size.
fn data(item: &Value) -> BTreeMap<String, String> {
    let mut data: BTreeMap<String, String> = item["data"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect();
    for (key, value) in item["binaryData"].as_object().into_iter().flatten() {
        let size = value.as_str().map_or(0, decoded_len);
        data.insert(key.clone(), format!("<binary, {size} bytes>"));
    }
    data
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::kubectl::base64_decode;
    use crate::testing::run;

    #[test]
    fn configmaps_list_their_keys_and_get_shows_values_with_binary_as_size() {
        let outcome = run(&["k8s", "configmap", "list", "--fields", "name,keys"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[0],
            json!({"name": "orders-config", "keys": ["DB_HOST", "FEATURES", "LOG_LEVEL"]})
        );

        let outcome = run(&[
            "k8s",
            "configmap",
            "get",
            "orders-config",
            "--fields",
            "data",
        ]);
        assert_eq!(outcome.json()["data"]["LOG_LEVEL"], "info");

        let binary = json!({"data": {"a": "x"}, "binaryData": {"blob": "AAECAwQF"}});
        assert_eq!(data(&binary)["blob"], "<binary, 6 bytes>");
        assert_eq!(base64_decode("aHVudGVyMg==").unwrap(), b"hunter2");
        assert!(base64_decode("not base64!").is_err());
    }
}
