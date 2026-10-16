//! `k8s storageclass`: how a claim's volume is made. `pvc get` prints a
//! claim's class with the same row.

pub(crate) mod list;

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::non_empty;

/// The annotation that makes a class the one a claim without one gets.
const DEFAULT: &str = "storageclass.kubernetes.io/is-default-class";

#[derive(Debug, Serialize, JsonSchema)]
pub struct StorageClassRow {
    name: String,
    /// What a claim naming no class gets.
    default: bool,
    /// disk.csi.azure.com, file.csi.azure.com…
    provisioner: Option<String>,
    /// The driver's: skuname (Premium_LRS, StandardSSD_ZRS…), cachingmode…
    parameters: BTreeMap<String, String>,
    /// Immediate, or WaitForFirstConsumer: Pending until a pod is scheduled.
    binding_mode: Option<String>,
    /// Whether a claim can be resized (allowVolumeExpansion).
    expansion: bool,
    /// Delete or Retain.
    reclaim: Option<String>,
}

pub(crate) fn row(item: &Value) -> StorageClassRow {
    StorageClassRow {
        name: item["metadata"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        default: item["metadata"]["annotations"][DEFAULT] == "true",
        provisioner: non_empty(&item["provisioner"]).map(str::to_owned),
        parameters: item["parameters"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect(),
        binding_mode: non_empty(&item["volumeBindingMode"]).map(str::to_owned),
        expansion: item["allowVolumeExpansion"] == true,
        reclaim: non_empty(&item["reclaimPolicy"]).map(str::to_owned),
    }
}
