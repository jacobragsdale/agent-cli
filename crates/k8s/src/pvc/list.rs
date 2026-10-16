use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, age, items, limited, non_empty};
use crate::pv::access_modes;

use super::used_by;

#[derive(clap::Args)]
pub struct PvcListArgs {
    /// Part of the claim name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PvcRow {
    /// `cluster/namespace/name`: what `pvc get` takes.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// Pending, Bound or Lost.
    status: Option<String>,
    /// What the volume holds; less than requested while a resize is pending.
    capacity: Option<String>,
    requested: Option<String>,
    /// RWO, ROX, RWX, RWOP.
    access_modes: Vec<String>,
    storage_class: Option<String>,
    /// The bound volume's id (cluster/name): what `pv get` takes.
    volume: Option<String>,
    /// The ids of the pods that mount it.
    used_by: Vec<String>,
    age: Option<String>,
}

fn pvc_list(ctx: &Ctx, args: PvcListArgs) -> Result<Vec<PvcRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "persistentvolumeclaims,pods", "-o", "json"])?;
    let rows = items(&listed)
        .filter(|item| item["kind"] == "PersistentVolumeClaim")
        .filter(|item| {
            args.name.as_deref().is_none_or(|part| {
                item["metadata"]["name"]
                    .as_str()
                    .is_some_and(|name| name.contains(part))
            })
        })
        .map(|item| {
            let spec = &item["spec"];
            PvcRow {
                id: target.id(item),
                name: item["metadata"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                namespace: target.row_namespace(item),
                status: non_empty(&item["status"]["phase"]).map(str::to_owned),
                capacity: non_empty(&item["status"]["capacity"]["storage"]).map(str::to_owned),
                requested: non_empty(&spec["resources"]["requests"]["storage"]).map(str::to_owned),
                access_modes: access_modes(&spec["accessModes"]),
                storage_class: non_empty(&spec["storageClassName"]).map(str::to_owned),
                volume: non_empty(&spec["volumeName"])
                    .map(|volume| format!("{}/{volume}", target.scope)),
                used_by: used_by(&listed, item)
                    .into_iter()
                    .map(|pod| target.id(pod))
                    .collect(),
                age: age(&item["metadata"]["creationTimestamp"]),
            }
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub PVC_LIST = ["k8s", "pvc", "list"], Read,
    "List persistent volume claims: status, size, storage class, pods mounting each",
    keywords: ["persistentvolumeclaim", "claims", "volumes", "disk", "storage", "bound", "pending", "mounted"],
    example: "k8s pvc list --cluster qa --namespace dev --fields id,status,capacity,used_by",
    run: pvc_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn claims_list_with_their_volume_and_the_pods_that_mount_them() {
        let outcome = run(&["k8s", "pvc", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"id": "qa/dev/orders-data", "name": "orders-data", "status": "Bound",
                "capacity": "32Gi", "requested": "64Gi", "access_modes": ["RWO"],
                "storage_class": "managed-csi", "volume": "qa/pvc-0a1b2c3d-1111-4000-8000-000000000001",
                "used_by": ["qa/dev/orders-api-7d9f5b-abc12"], "age": rows[0]["age"]})
        );
        assert_eq!(rows[2]["status"], "Pending");
        assert!(rows[2].get("used_by").is_none(), "{}", rows[2]);

        let outcome = run(&["k8s", "pvc", "list", "scratch", "--fields", "id"]);
        assert_eq!(outcome.json(), json!([{"id": "qa/dev/scratch"}]));
    }
}
