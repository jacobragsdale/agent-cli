//! `k8s serviceaccount …`: who a pod runs as, the Azure identity workload
//! identity maps it to (its client id, which `arm identity get` takes), and
//! the pods running as it.

pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{Target, non_empty};

/// The annotation workload identity reads: the Entra app or user-assigned
/// identity whose token the pods get.
const CLIENT_ID: &str = "azure.workload.identity/client-id";

#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceAccountRow {
    /// `cluster/namespace/name`: what `serviceaccount get` and `access list --as` take.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// The Azure identity's client id workload identity signs its pods in
    /// as: what `arm identity get` takes.
    client_id: Option<String>,
    /// The ids of the secrets its pods pull images with.
    pull_secrets: Vec<String>,
    /// Whether its token is mounted into pods that do not say; absent is
    /// Kubernetes' default, true.
    automount: Option<bool>,
    /// The ids of the pods running as it.
    pods: Vec<String>,
}

pub(crate) fn row(target: &Target, item: &Value, pods: &[&Value]) -> Option<ServiceAccountRow> {
    let name = non_empty(&item["metadata"]["name"])?;
    let namespace = &item["metadata"]["namespace"];
    let in_namespace = |held: &str| {
        format!(
            "{}/{}/{held}",
            target.scope,
            namespace.as_str().unwrap_or_default()
        )
    };
    Some(ServiceAccountRow {
        id: target.id(item),
        name: name.to_owned(),
        namespace: target.row_namespace(item),
        client_id: non_empty(&item["metadata"]["annotations"][CLIENT_ID]).map(str::to_owned),
        pull_secrets: item["imagePullSecrets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|held| non_empty(&held["name"]).map(in_namespace))
            .collect(),
        automount: item["automountServiceAccountToken"].as_bool(),
        pods: pods
            .iter()
            .filter(|pod| {
                &pod["metadata"]["namespace"] == namespace
                    && non_empty(&pod["spec"]["serviceAccountName"]).unwrap_or("default") == name
            })
            .map(|pod| target.id(pod))
            .collect(),
    })
}
