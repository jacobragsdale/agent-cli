//! `k8s pvc …`: a claim joined to the pods that mount it; `pvc get` adds
//! its volume, class, attachment and events.

pub(crate) mod get;
pub(crate) mod list;

use serde_json::Value;

use crate::kubectl::items;

/// The pods in `listed` that mount `claim`.
pub(crate) fn used_by<'a>(listed: &'a Value, claim: &Value) -> Vec<&'a Value> {
    let namespace = &claim["metadata"]["namespace"];
    let name = &claim["metadata"]["name"];
    items(listed)
        .filter(|pod| pod["kind"] == "Pod" && &pod["metadata"]["namespace"] == namespace)
        .filter(|pod| {
            pod["spec"]["volumes"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|volume| &volume["persistentVolumeClaim"]["claimName"] == name)
        })
        .collect()
}
