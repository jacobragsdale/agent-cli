//! `k8s statefulset …`: ordered pods with a claim each, read from kubectl's
//! JSON.

pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod restart;
pub(crate) mod scale;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{Target, age, non_empty};
use crate::pod::images;

#[derive(Debug, Serialize, JsonSchema)]
pub struct StatefulSetRow {
    /// `cluster/namespace/name`: what `statefulset get` takes.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// Pods ready of pods wanted: 0/1.
    ready: String,
    images: Vec<String>,
    /// The claim templates' names; `get` lists each pod's claim.
    claim_templates: Vec<String>,
    age: Option<String>,
}

fn row(target: &Target, item: &Value) -> Option<StatefulSetRow> {
    Some(StatefulSetRow {
        id: target.id(item),
        name: item["metadata"]["name"].as_str()?.to_owned(),
        namespace: target.row_namespace(item),
        ready: format!(
            "{}/{}",
            item["status"]["readyReplicas"].as_i64().unwrap_or(0),
            wanted(item)
        ),
        images: images(&item["spec"]["template"]["spec"]),
        claim_templates: templates(item)
            .filter_map(|claim| non_empty(&claim["metadata"]["name"]).map(str::to_owned))
            .collect(),
        age: age(&item["metadata"]["creationTimestamp"]),
    })
}

fn wanted(item: &Value) -> i64 {
    item["spec"]["replicas"].as_i64().unwrap_or(1)
}

fn templates(item: &Value) -> impl Iterator<Item = &Value> {
    item["spec"]["volumeClaimTemplates"]
        .as_array()
        .into_iter()
        .flatten()
}
