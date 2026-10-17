//! `k8s networkpolicy …`: what a policy selects and allows, in words, with
//! the default deny a policy makes by naming a direction it allows nothing in.

pub(crate) mod diff;
pub(crate) mod get;
pub(crate) mod list;

use serde_json::Value;

use crate::shape::selector_text;

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

/// The pods a policy applies to, in words.
fn pod_selector(policy: &Value) -> String {
    selector_text(&policy["spec"]["podSelector"]).unwrap_or_else(|| "every pod".to_owned())
}
