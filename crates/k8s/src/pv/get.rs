use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::At;
use crate::shape::{EventRow, events_for};

use super::{PvRow, attached_to, node_affinity, row};

#[derive(clap::Args)]
pub struct PvGetArgs {
    /// The volume: its id (cluster/name) or name
    pv: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PvDetail {
    #[serde(flatten)]
    pv: PvRow,
    /// Its required node affinity in words: the zone a disk lives in.
    node_affinity: Vec<String>,
    /// The node's id (cluster/name) its VolumeAttachment names.
    attached_to: Option<String>,
    /// The five newest events about it, newest first.
    events: Vec<EventRow>,
}

fn pv_get(ctx: &Ctx, args: PvGetArgs) -> Result<PvDetail> {
    let (target, name) = args.at.named_cluster(ctx, &args.pv)?;
    let item = target.json(ctx, &["get", "persistentvolume", &name, "-o", "json"])?;
    let attachments = target.json(ctx, &["get", "volumeattachments", "-o", "json"])?;
    Ok(PvDetail {
        pv: row(&target, &item),
        node_affinity: node_affinity(&item),
        attached_to: attached_to(&target, &attachments, &name),
        events: events_for(ctx, &target, &item),
    })
}

command! {
    pub PV_GET = ["k8s", "pv", "get"], Read,
    "Show a persistent volume: Azure disk id, zone affinity, attachment, events",
    keywords: ["persistentvolume", "volumeattachment", "csi", "handle", "arm"],
    example: "k8s pv get qa/pvc-0a1b2c3d-1111-4000-8000-000000000001 --fields disk,zone,attached_to",
    run: pv_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn a_volume_names_its_disk_its_zone_and_the_node_holding_it() {
        let outcome = k8s(&[
            "k8s",
            "pv",
            "get",
            "qa/pvc-0a1b2c3d-2222-4000-8000-000000000002",
            "--fields",
            "id,claim,zone,node_affinity,attached_to",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "qa/pvc-0a1b2c3d-2222-4000-8000-000000000002", "claim": "qa/uat/orders-data",
                "zone": "westeurope-1",
                "node_affinity": ["topology.disk.csi.azure.com/zone In westeurope-1"],
                "attached_to": "qa/aks-np1-vmss000001"})
        );

        let outcome = k8s(&["k8s", "pv", "get", "gone", "--cluster", "qa"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("hint: agent-cli k8s pv list"),
            "{}",
            outcome.stderr
        );
    }
}
