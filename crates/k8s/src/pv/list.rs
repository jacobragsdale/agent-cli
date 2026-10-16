use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, items, limited};

use super::{PvRow, row};

#[derive(clap::Args)]
pub struct PvListArgs {
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn pv_list(ctx: &Ctx, args: PvListArgs) -> Result<Vec<PvRow>> {
    let target = args.at.cluster_wide(ctx)?;
    let listed = target.json(ctx, &["get", "persistentvolumes", "-o", "json"])?;
    let rows = items(&listed).map(|item| row(&target, item)).collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub PV_LIST = ["k8s", "pv", "list"], Read,
    "List persistent volumes: claim, capacity, reclaim policy, Azure disk or share",
    keywords: ["persistentvolume", "volumes", "released", "retain", "csi", "storage"],
    example: "k8s pv list --cluster qa --fields id,status,claim,disk",
    run: pv_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn volumes_list_cluster_wide_with_their_claim_and_what_holds_their_data() {
        let outcome = k8s(&["k8s", "pv", "list", "--cluster", "qa"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"id": "qa/pvc-0a1b2c3d-1111-4000-8000-000000000001", "status": "Bound",
                "claim": "qa/dev/orders-data", "capacity": "64Gi", "access_modes": ["RWO"],
                "reclaim": "Delete", "storage_class": "managed-csi", "driver": "disk.csi.azure.com",
                "disk": "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/mc_contoso-qa/providers/Microsoft.Compute/disks/pvc-0a1b2c3d-1111-4000-8000-000000000001",
                "zone": "westeurope-1", "age": rows[0]["age"]})
        );
        assert_eq!(
            rows[2]["share"],
            json!({"resource_group": "mc_contoso-qa", "account": "fcontosoqa", "name": "pvc-reports"})
        );
        assert!(rows[2].get("disk").is_none(), "{}", rows[2]);
    }
}
