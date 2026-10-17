//! `k8s ingress …`: ingresses, and in `get` the services behind each rule.

pub(crate) mod diff;
pub(crate) mod get;
pub(crate) mod list;

use serde_json::Value;

use crate::kubectl::non_empty;

/// Its class: `ingressClassName`, else the annotation older ingresses set.
fn class(ingress: &Value) -> Option<String> {
    non_empty(&ingress["spec"]["ingressClassName"])
        .or_else(|| non_empty(&ingress["metadata"]["annotations"]["kubernetes.io/ingress.class"]))
        .map(str::to_owned)
}

/// Where the controller serves it: its load balancer's IPs or host names.
fn address(ingress: &Value) -> Vec<String> {
    ingress["status"]["loadBalancer"]["ingress"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|held| non_empty(&held["ip"]).or_else(|| non_empty(&held["hostname"])))
        .map(str::to_owned)
        .collect()
}
