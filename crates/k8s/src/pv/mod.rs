//! `k8s pv …`: a PersistentVolume as the Azure CSI drivers hold it, with
//! the disk's ARM id or the file share it names, its zone and the node a
//! VolumeAttachment says it is attached to. `pvc get` prints the same row.

pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{Target, age, items, non_empty};

const AZURE_DISK: &str = "disk.csi.azure.com";
const AZURE_FILE: &str = "file.csi.azure.com";

#[derive(Debug, Serialize, JsonSchema)]
pub struct PvRow {
    /// `cluster/name`: what `pv get` takes.
    id: String,
    /// Available, Bound, Released or Failed.
    status: Option<String>,
    /// The claim's id (cluster/namespace/name): what `pvc get` takes.
    claim: Option<String>,
    capacity: Option<String>,
    /// RWO, ROX, RWX, RWOP.
    access_modes: Vec<String>,
    /// Delete or Retain: what happens to the disk when the claim goes.
    reclaim: Option<String>,
    storage_class: Option<String>,
    /// The CSI driver: disk.csi.azure.com, file.csi.azure.com…
    driver: Option<String>,
    /// The Azure managed disk's ARM id: what `arm resource get` takes.
    disk: Option<String>,
    /// The Azure file share, as the volume handle names it.
    share: Option<Share>,
    /// The zone its node affinity pins it to; a pod elsewhere cannot mount it.
    zone: Option<String>,
    age: Option<String>,
}

/// What an Azure Files volume handle holds (`rg#account#share#…`): not an
/// ARM id, so none is guessed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Share {
    /// Empty in the handle when it is the cluster's node resource group.
    resource_group: Option<String>,
    account: String,
    name: String,
}

/// The row of a PV. `target` may be a claim's, in a namespace: the id is
/// built from its scope, as a cluster-scoped kind's is.
pub(crate) fn row(target: &Target, item: &Value) -> PvRow {
    let spec = &item["spec"];
    let claim = &spec["claimRef"];
    let driver = non_empty(&spec["csi"]["driver"]);
    let handle = non_empty(&spec["csi"]["volumeHandle"]);
    PvRow {
        id: format!(
            "{}/{}",
            target.scope,
            item["metadata"]["name"].as_str().unwrap_or_default()
        ),
        status: non_empty(&item["status"]["phase"]).map(str::to_owned),
        claim: non_empty(&claim["name"]).map(|name| {
            let namespace = claim["namespace"].as_str().unwrap_or_default();
            format!("{}/{namespace}/{name}", target.scope)
        }),
        capacity: non_empty(&spec["capacity"]["storage"]).map(str::to_owned),
        access_modes: access_modes(&spec["accessModes"]),
        reclaim: non_empty(&spec["persistentVolumeReclaimPolicy"]).map(str::to_owned),
        storage_class: non_empty(&spec["storageClassName"]).map(str::to_owned),
        driver: driver.map(str::to_owned),
        disk: handle
            .filter(|handle| driver == Some(AZURE_DISK) && handle.starts_with("/subscriptions/"))
            .map(str::to_owned),
        share: handle
            .filter(|_| driver == Some(AZURE_FILE))
            .and_then(share),
        zone: affinity(item)
            .find(|expression| {
                expression["key"]
                    .as_str()
                    .is_some_and(|key| key.ends_with("/zone"))
            })
            .and_then(|expression| non_empty(&expression["values"][0]))
            .map(str::to_owned),
        age: age(&item["metadata"]["creationTimestamp"]),
    }
}

fn share(handle: &str) -> Option<Share> {
    let mut parts = handle.split('#');
    let resource_group = parts.next()?;
    let (account, name) = (parts.next()?, parts.next()?);
    (!account.is_empty() && !name.is_empty()).then(|| Share {
        resource_group: (!resource_group.is_empty()).then(|| resource_group.to_owned()),
        account: account.to_owned(),
        name: name.to_owned(),
    })
}

/// The match expressions of the PV's required node affinity.
fn affinity(item: &Value) -> impl Iterator<Item = &Value> {
    item["spec"]["nodeAffinity"]["required"]["nodeSelectorTerms"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|term| term["matchExpressions"].as_array().into_iter().flatten())
}

/// The node affinity in words: `topology.disk.csi.azure.com/zone In westeurope-1`.
pub(crate) fn node_affinity(item: &Value) -> Vec<String> {
    affinity(item)
        .map(|expression| {
            let values: Vec<&str> = expression["values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            format!(
                "{} {} {}",
                expression["key"].as_str().unwrap_or_default(),
                expression["operator"].as_str().unwrap_or_default(),
                values.join(",")
            )
            .trim_end()
            .to_owned()
        })
        .collect()
}

/// Access modes as kubectl's ACCESS MODES column abbreviates them.
pub(crate) fn access_modes(modes: &Value) -> Vec<String> {
    modes
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|mode| {
            match mode {
                "ReadWriteOnce" => "RWO",
                "ReadOnlyMany" => "ROX",
                "ReadWriteMany" => "RWX",
                "ReadWriteOncePod" => "RWOP",
                other => other,
            }
            .to_owned()
        })
        .collect()
}

/// The node (its id, `cluster/name`) a VolumeAttachment in `listed` says
/// holds the PV `pv`. A disk stays attached to a node that went away until
/// the attach-detach controller lets go, which is a Multi-Attach error for
/// a pod on any other node.
pub(crate) fn attached_to(target: &Target, listed: &Value, pv: &str) -> Option<String> {
    items(listed)
        .filter(|item| item["kind"] == "VolumeAttachment")
        .filter(|item| item["spec"]["source"]["persistentVolumeName"] == pv)
        .find(|item| item["status"]["attached"] == true)
        .and_then(|item| non_empty(&item["spec"]["nodeName"]))
        .map(|node| format!("{}/{node}", target.scope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_share_handle_reads_as_its_group_account_and_share_never_an_arm_id() {
        let held = share("mc_contoso-qa#fcontosoqa#pvc-reports###web").unwrap();
        assert_eq!(held.resource_group.as_deref(), Some("mc_contoso-qa"));
        assert_eq!(
            (held.account.as_str(), held.name.as_str()),
            ("fcontosoqa", "pvc-reports")
        );
        assert!(
            share("#fcontosoqa#reports")
                .unwrap()
                .resource_group
                .is_none()
        );
        assert!(share("not-a-handle").is_none());
    }
}
