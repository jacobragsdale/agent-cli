use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, items, limited};

use super::{StorageClassRow, row};

#[derive(clap::Args)]
pub struct StorageclassListArgs {
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn storageclass_list(ctx: &Ctx, args: StorageclassListArgs) -> Result<Vec<StorageClassRow>> {
    let target = args.at.cluster_wide(ctx)?;
    let listed = target.json(ctx, &["get", "storageclasses", "-o", "json"])?;
    let rows = items(&listed).map(row).collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub STORAGECLASS_LIST = ["k8s", "storageclass", "list"], Read,
    "List storage classes: default, provisioner, SKU, binding mode, volume expansion",
    keywords: ["sc", "sku", "premium", "provisioner", "expansion", "resize", "allow", "default", "reclaim", "waitforfirstconsumer"],
    example: "k8s storageclass list --cluster qa --fields name,default,provisioner,expansion",
    run: storageclass_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn storage_classes_say_which_is_the_default_and_whether_claims_can_grow() {
        let outcome = k8s(&["k8s", "storageclass", "list", "--cluster", "qa"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"name": "managed-csi", "default": true, "provisioner": "disk.csi.azure.com",
                    "parameters": {"skuname": "StandardSSD_LRS"}, "binding_mode": "WaitForFirstConsumer",
                    "expansion": true, "reclaim": "Delete"},
                {"name": "azurefile-csi", "default": false, "provisioner": "file.csi.azure.com",
                    "parameters": {"skuName": "Standard_LRS"}, "binding_mode": "Immediate",
                    "expansion": false, "reclaim": "Delete"},
            ])
        );
    }
}
