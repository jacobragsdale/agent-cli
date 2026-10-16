//! `k8s networkpolicy …`: what a policy selects and allows, in words, with
//! the default deny a policy makes by naming a direction it allows nothing in.

pub(crate) mod get;
pub(crate) mod list;

use serde_json::Value;

/// The directions a policy governs: `policyTypes`, else Ingress, and Egress
/// too when it has egress rules, as the API defaults them.
fn types(policy: &Value) -> Vec<String> {
    let spec = &policy["spec"];
    match spec["policyTypes"].as_array() {
        Some(types) => types
            .iter()
            .filter_map(|held| held.as_str().map(str::to_owned))
            .collect(),
        None if spec["egress"].is_array() => vec!["Ingress".to_owned(), "Egress".to_owned()],
        None => vec!["Ingress".to_owned()],
    }
}

/// The rules of one direction: `ingress` or `egress`.
fn rules<'a>(policy: &'a Value, direction: &str) -> &'a [Value] {
    policy["spec"][direction]
        .as_array()
        .map_or(&[], Vec::as_slice)
}

/// Whether it denies everything in a direction it governs: it selects pods
/// and lists no rule there.
fn default_deny(policy: &Value) -> bool {
    types(policy)
        .iter()
        .any(|kind| rules(policy, &kind.to_ascii_lowercase()).is_empty())
}

/// A label selector as `kubectl` writes one: `app=api,tier in (web),!canary`.
/// None for the empty selector, which selects everything.
fn selector_text(selector: &Value) -> Option<String> {
    let mut terms: Vec<String> = selector["matchLabels"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or_default()))
        .collect();
    for expression in selector["matchExpressions"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let key = expression["key"].as_str().unwrap_or_default();
        let values: Vec<&str> = expression["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        terms.push(match expression["operator"].as_str().unwrap_or_default() {
            "In" => format!("{key} in ({})", values.join(",")),
            "NotIn" => format!("{key} notin ({})", values.join(",")),
            "DoesNotExist" => format!("!{key}"),
            _ => key.to_owned(),
        });
    }
    (!terms.is_empty()).then(|| terms.join(","))
}

/// The pods a policy applies to, in words.
fn pod_selector(policy: &Value) -> String {
    selector_text(&policy["spec"]["podSelector"]).unwrap_or_else(|| "every pod".to_owned())
}
