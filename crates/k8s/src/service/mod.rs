//! `k8s service …`: a service joined to the EndpointSlices that say which
//! pods answer for it. `ingress get` counts its backends' endpoints here too.

pub(crate) mod diff;
pub(crate) mod get;
pub(crate) mod list;

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{Target, items, non_empty};

/// The label an EndpointSlice names its service by.
const SERVICE_NAME: &str = "kubernetes.io/service-name";

/// One address that answers for a service.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Endpoint {
    /// The pod's id: what `k8s pod get` takes.
    pod: Option<String>,
    ip: Option<String>,
    pub(crate) ready: bool,
    node: Option<String>,
    zone: Option<String>,
}

/// The endpoints of `service` in the slices `listed` holds, once per pod
/// (a dual-stack pod is in an IPv4 slice and an IPv6 one).
pub(crate) fn endpoints(target: &Target, listed: &Value, service: &Value) -> Vec<Endpoint> {
    let namespace = &service["metadata"]["namespace"];
    let name = &service["metadata"]["name"];
    let mut seen = BTreeSet::new();
    items(listed)
        .filter(|item| {
            item["kind"] == "EndpointSlice"
                && &item["metadata"]["namespace"] == namespace
                && &item["metadata"]["labels"][SERVICE_NAME] == name
        })
        .flat_map(|slice| slice["endpoints"].as_array().into_iter().flatten())
        .filter_map(|endpoint| {
            let reference = &endpoint["targetRef"];
            let pod = (reference["kind"] == "Pod")
                .then(|| non_empty(&reference["name"]))
                .flatten()
                .map(|pod| target.id(&reference_item(namespace, pod)));
            let ip = non_empty(&endpoint["addresses"][0]).map(str::to_owned);
            seen.insert(pod.clone().or_else(|| ip.clone()))
                .then(|| Endpoint {
                    pod,
                    ip,
                    // An unknown state reads as ready, as the API asks of consumers.
                    ready: endpoint["conditions"]["ready"].as_bool() != Some(false),
                    node: non_empty(&endpoint["nodeName"]).map(str::to_owned),
                    zone: non_empty(&endpoint["zone"]).map(str::to_owned),
                })
        })
        .collect()
}

/// Ready of all, as `pod list` says a pod's containers: `2/3`.
pub(crate) fn ready(endpoints: &[Endpoint]) -> String {
    let ready = endpoints.iter().filter(|endpoint| endpoint.ready).count();
    format!("{ready}/{}", endpoints.len())
}

/// The selector as `-l` and `pod list --label` take it: `app=api,tier=web`.
/// None when it has none, and its endpoints are whatever slices were made
/// by hand.
fn selector(service: &Value) -> Option<String> {
    let pairs: Vec<String> = service["spec"]["selector"]
        .as_object()?
        .iter()
        .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or_default()))
        .collect();
    (!pairs.is_empty()).then(|| pairs.join(","))
}

/// The addresses it is reached at from outside the cluster: a load
/// balancer's IPs or host names, external IPs, an ExternalName's host.
fn external(service: &Value) -> Vec<String> {
    let spec = &service["spec"];
    let balanced = service["status"]["loadBalancer"]["ingress"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|ingress| {
            non_empty(&ingress["ip"]).or_else(|| non_empty(&ingress["hostname"]))
        });
    let listed = spec["externalIPs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(non_empty);
    balanced
        .chain(listed)
        .chain(non_empty(&spec["externalName"]))
        .map(str::to_owned)
        .collect()
}

fn kind(service: &Value) -> String {
    service["spec"]["type"]
        .as_str()
        .unwrap_or("ClusterIP")
        .to_owned()
}

fn reference_item(namespace: &Value, name: &str) -> Value {
    serde_json::json!({"metadata": {"namespace": namespace, "name": name}})
}
