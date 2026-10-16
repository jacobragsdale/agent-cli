//! What gets an object into shape for an agent, shared by the curated
//! commands and the generic `k8s object`: its recent events, its manifest
//! without the bookkeeping, and the listing to try when it is not there.

use std::cmp::Reverse;

use agent_cli_core::Ctx;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::kubectl::{Target, items, non_empty};

/// The kinds with a list of their own, as kubectl's errors name them
/// (plural), and the resource word of `agent-cli k8s RESOURCE list`.
const CURATED: [(&str, &str); 8] = [
    ("pods", "pod"),
    ("deployments", "deployment"),
    ("configmaps", "configmap"),
    ("secrets", "secret"),
    ("events", "event"),
    ("services", "service"),
    ("ingresses", "ingress"),
    ("networkpolicies", "networkpolicy"),
];

/// What `kubectl apply` keeps on the object: a copy of the whole manifest.
const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

#[derive(Debug, Serialize, JsonSchema)]
pub struct EventRow {
    /// Normal or Warning.
    #[serde(rename = "type")]
    kind: String,
    reason: String,
    /// What it is about: Pod/orders-api-7d9f5b-abc12.
    object: String,
    message: String,
    count: i64,
    /// RFC 3339.
    last_seen: Option<String>,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
}

/// An event as a row, with when it was last seen to sort and filter on.
pub(crate) fn event_row(target: &Target, item: &Value) -> (Option<OffsetDateTime>, EventRow) {
    let stamp = |value: &Value| {
        value
            .as_str()
            .and_then(|raw| OffsetDateTime::parse(raw, &Rfc3339).ok())
    };
    // Newer event APIs write eventTime and series in place of the old stamps
    // and count.
    let last = stamp(&item["lastTimestamp"])
        .or_else(|| stamp(&item["series"]["lastObservedTime"]))
        .or_else(|| stamp(&item["eventTime"]))
        .or_else(|| stamp(&item["metadata"]["creationTimestamp"]));
    let involved = &item["involvedObject"];
    let row = EventRow {
        kind: item["type"].as_str().unwrap_or("Normal").to_owned(),
        reason: item["reason"].as_str().unwrap_or_default().to_owned(),
        object: format!(
            "{}/{}",
            involved["kind"].as_str().unwrap_or_default(),
            involved["name"].as_str().unwrap_or_default()
        ),
        message: item["message"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned(),
        count: item["count"]
            .as_i64()
            .or_else(|| item["series"]["count"].as_i64())
            .unwrap_or(1),
        last_seen: last.map(agent_cli_core::utc_time),
        namespace: target.row_namespace(item),
    };
    (last, row)
}

/// The five newest events about `item`, matched by uid as `kubectl
/// describe` matches them: what an agent reading one object asks next. A
/// failed read is a note, never the command's failure.
pub(crate) fn events_for(ctx: &Ctx, target: &Target, item: &Value) -> Vec<EventRow> {
    let Some(uid) = non_empty(&item["metadata"]["uid"]) else {
        return Vec::new();
    };
    let selector = format!("involvedObject.uid={uid}");
    let mut argv = vec!["get", "events", "-o", "json", "--field-selector", &selector];
    // A cluster-scoped object's events sit in `default`, or wherever its
    // controller wrote them.
    if target.cluster_scoped {
        argv.push("--all-namespaces");
    }
    match target.json(ctx, &argv) {
        Ok(listed) => {
            let mut rows: Vec<_> = items(&listed).map(|item| event_row(target, item)).collect();
            rows.sort_by_key(|(last, _)| Reverse(*last));
            rows.into_iter().take(5).map(|(_, row)| row).collect()
        }
        Err(error) => {
            ctx.note(format!("[events not read: {error:#}]"));
            Vec::new()
        }
    }
}

/// Whether a label selector (`matchLabels`, and `matchExpressions` with In,
/// NotIn, Exists and DoesNotExist) selects an object with `labels`. An empty
/// selector selects everything, as a NetworkPolicy's `podSelector: {}` does.
pub(crate) fn selects(selector: &Value, labels: &Value) -> bool {
    let label = |key: &str| labels[key].as_str();
    let pairs = selector["matchLabels"]
        .as_object()
        .into_iter()
        .flatten()
        .all(|(key, value)| label(key) == value.as_str());
    pairs
        && selector["matchExpressions"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|expression| {
                let held = label(expression["key"].as_str().unwrap_or_default());
                let listed = |held: &str| {
                    expression["values"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|value| value.as_str() == Some(held))
                };
                match expression["operator"].as_str() {
                    Some("In") => held.is_some_and(listed),
                    Some("NotIn") => held.is_none_or(|held| !listed(held)),
                    Some("Exists") => held.is_some(),
                    Some("DoesNotExist") => held.is_none(),
                    _ => false,
                }
            })
}

/// A manifest without what only the API server's bookkeeping needs
/// (`managedFields`, the last-applied copy of itself), status kept.
pub(crate) fn trim(mut manifest: Value) -> Value {
    if let Some(metadata) = manifest.get_mut("metadata").and_then(Value::as_object_mut) {
        metadata.remove("managedFields");
        if let Some(annotations) = metadata
            .get_mut("annotations")
            .and_then(Value::as_object_mut)
        {
            annotations.remove(LAST_APPLIED);
            if annotations.is_empty() {
                metadata.remove("annotations");
            }
        }
    }
    manifest
}

/// The listing that shows what is there, for kubectl's `pods "x" not found`
/// or `certificates.cert-manager.io "x" not found`: a curated kind's own
/// list, else `k8s object list` of the resource as kubectl named it.
pub(crate) fn listing_for(message: &str) -> Option<String> {
    let (before, _) = message.split_once(" \"")?;
    let resource = before.rsplit(' ').next()?;
    let plural = resource.split('.').next()?;
    let word = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-';
    if plural.is_empty() || !resource.chars().all(word) {
        return None;
    }
    Some(match CURATED.iter().find(|(held, _)| *held == plural) {
        Some((_, kind)) => format!("agent-cli k8s {kind} list"),
        None => format!("agent-cli k8s object list {resource}"),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_missing_object_points_at_its_curated_list_or_the_generic_one() {
        let hint = |message| listing_for(message);
        assert_eq!(
            hint("Error from server (NotFound): pods \"x\" not found").as_deref(),
            Some("agent-cli k8s pod list")
        );
        assert_eq!(
            hint("Error from server (NotFound): deployments.apps \"x\" not found").as_deref(),
            Some("agent-cli k8s deployment list")
        );
        assert_eq!(
            hint("Error from server (NotFound): certificates.cert-manager.io \"x\" not found")
                .as_deref(),
            Some("agent-cli k8s object list certificates.cert-manager.io")
        );
        assert_eq!(
            hint("the server could not find the requested resource"),
            None
        );
    }

    #[test]
    fn a_selector_reads_match_labels_and_every_expression_operator() {
        let labels = json!({"app": "redis", "tier": "cache"});
        let selects = |selector| selects(&selector, &labels);
        assert!(selects(json!({})), "an empty selector selects every pod");
        assert!(selects(json!({"matchLabels": {"app": "redis"}})));
        assert!(!selects(
            json!({"matchLabels": {"app": "redis", "tier": "db"}})
        ));
        let expression = |operator: &str, key: &str, values: &[&str]| json!({"matchExpressions": [{"key": key, "operator": operator, "values": values}]});
        assert!(selects(expression("In", "app", &["api", "redis"])));
        assert!(!selects(expression("In", "zone", &["a"])));
        assert!(selects(expression("NotIn", "app", &["api"])));
        assert!(
            selects(expression("NotIn", "zone", &["a"])),
            "NotIn holds without the label"
        );
        assert!(!selects(expression("NotIn", "app", &["redis"])));
        assert!(selects(expression("Exists", "tier", &[])));
        assert!(!selects(expression("DoesNotExist", "tier", &[])));
        assert!(selects(expression("DoesNotExist", "zone", &[])));
        assert!(!selects(expression("Gt", "app", &[])));
    }

    #[test]
    fn trimming_drops_the_bookkeeping_and_keeps_the_status() {
        let trimmed = trim(json!({
            "metadata": {"name": "x", "managedFields": [{"manager": "kubectl"}],
                "annotations": {LAST_APPLIED: "{}", "team": "orders"}},
            "status": {"phase": "Bound"},
        }));
        assert_eq!(
            trimmed,
            json!({"metadata": {"name": "x", "annotations": {"team": "orders"}},
                "status": {"phase": "Bound"}})
        );
        let bare = trim(json!({"metadata": {"name": "x", "annotations": {LAST_APPLIED: "{}"}}}));
        assert_eq!(bare, json!({"metadata": {"name": "x"}}));
    }
}
