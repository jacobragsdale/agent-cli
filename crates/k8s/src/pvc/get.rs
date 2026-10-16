use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, Target, items, non_empty};
use crate::pv::{PvRow, access_modes, attached_to};
use crate::shape::{Condition, EventRow, conditions, newest};
use crate::storageclass::StorageClassRow;

use super::used_by;

#[derive(clap::Args)]
pub struct PvcGetArgs {
    /// The claim: its id (cluster/namespace/name), namespace/name, or name
    pvc: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PvcDetail {
    id: String,
    name: String,
    namespace: String,
    /// Pending, Bound or Lost.
    status: Option<String>,
    capacity: Option<String>,
    requested: Option<String>,
    /// RWO, ROX, RWX, RWOP.
    access_modes: Vec<String>,
    /// Resizing, FileSystemResizePending…
    conditions: Vec<Condition>,
    /// The bound volume: its Azure disk's ARM id or file share, its zone.
    pv: Option<PvRow>,
    storage_class: Option<String>,
    /// The class itself: provisioner, SKU, binding mode, expansion. Absent
    /// when no class has the name.
    class: Option<StorageClassRow>,
    /// The node's id (cluster/name) its volume is attached to.
    attached_to: Option<String>,
    /// The ids of the pods that mount it.
    used_by: Vec<String>,
    /// The five newest events about the claim or its volume (Multi-Attach,
    /// ProvisioningFailed, resizes), newest first.
    events: Vec<EventRow>,
}

fn pvc_get(ctx: &Ctx, args: PvcGetArgs) -> Result<PvcDetail> {
    let (target, name) = args.at.named(ctx, &args.pvc)?;
    let item = target.json(ctx, &["get", "persistentvolumeclaim", &name, "-o", "json"])?;
    // ponytail: every volume, class, attachment, pod and event of the
    // namespace in one call; `-l` and field selectors per kind if that grows
    // to thousands.
    let listed = target.json(
        ctx,
        &[
            "get",
            "persistentvolumes,storageclasses,volumeattachments,pods,events",
            "-o",
            "json",
        ],
    )?;
    let spec = &item["spec"];
    let named = |kind: &str, wanted: &Value| {
        items(&listed).find(|held| held["kind"] == kind && &held["metadata"]["name"] == wanted)
    };
    let volume = non_empty(&spec["volumeName"]);
    let class = named("StorageClass", &spec["storageClassName"]).map(crate::storageclass::row);
    let pods = used_by(&listed, &item);
    let attached = volume.and_then(|volume| attached_to(&target, &listed, volume));
    if let Some(next) = next(&target, &item, class.is_some(), &pods, attached.as_deref()) {
        ctx.note(format!("[next: {next}]"));
    }
    Ok(PvcDetail {
        id: target.id(&item),
        name,
        namespace: target.namespace.clone().unwrap_or_default(),
        status: non_empty(&item["status"]["phase"]).map(str::to_owned),
        capacity: non_empty(&item["status"]["capacity"]["storage"]).map(str::to_owned),
        requested: non_empty(&spec["resources"]["requests"]["storage"]).map(str::to_owned),
        access_modes: access_modes(&spec["accessModes"]),
        conditions: conditions(&item),
        pv: volume
            .and_then(|volume| named("PersistentVolume", &Value::from(volume)))
            .map(|pv| crate::pv::row(&target, pv)),
        storage_class: non_empty(&spec["storageClassName"]).map(str::to_owned),
        class,
        attached_to: attached,
        used_by: pods.iter().map(|pod| target.id(pod)).collect(),
        events: events(&target, &listed, &item, volume),
    })
}

/// The claim's own events and those naming its volume: a Multi-Attach or
/// a failed mount is reported on the pod, with the volume's name in quotes.
fn events(target: &Target, listed: &Value, item: &Value, volume: Option<&str>) -> Vec<EventRow> {
    let quoted = volume.map(|volume| format!("\"{volume}\""));
    newest(
        target,
        items(listed).filter(|held| {
            held["kind"] == "Event"
                && (held["involvedObject"]["uid"] == item["metadata"]["uid"]
                    || quoted.as_deref().is_some_and(|quoted| {
                        held["message"]
                            .as_str()
                            .is_some_and(|message| message.contains(quoted))
                    }))
        }),
    )
}

/// What a stuck claim asks next. Pending: the class when it is missing
/// or no pod waits on it, else the pod (whose scheduling says why). A disk
/// attached to a node other than the one its pod waits on: that node,
/// which has not let go of it (a Multi-Attach error).
fn next(
    target: &Target,
    item: &Value,
    has_class: bool,
    pods: &[&Value],
    attached: Option<&str>,
) -> Option<String> {
    if item["status"]["phase"] == "Pending" {
        return Some(match pods.first() {
            Some(pod) if has_class => format!("agent-cli k8s pod get {}", target.id(pod)),
            _ => format!("agent-cli k8s storageclass list --cluster {}", target.scope),
        });
    }
    let attached = attached?;
    let node = attached.split_once('/').map_or(attached, |(_, node)| node);
    pods.iter()
        .any(|pod| {
            pod["status"]["phase"] != "Running"
                && non_empty(&pod["spec"]["nodeName"]).is_some_and(|held| held != node)
        })
        .then(|| format!("agent-cli k8s node get {attached}"))
}

command! {
    pub PVC_GET = ["k8s", "pvc", "get"], Read,
    "Show a volume claim: Azure disk or share, class, attachment, mounts, events",
    keywords: ["persistentvolumeclaim", "multi-attach", "attach", "attached", "stuck", "pending", "provisioning", "resize", "disk", "share", "node"],
    example: "k8s pvc get qa/dev/orders-data --fields status,pv,attached_to,used_by,events",
    run: pvc_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{k8s, run};

    #[test]
    fn a_bound_claim_joins_its_disk_class_attachment_and_pods() {
        let outcome = run(&["k8s", "pvc", "get", "orders-data"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let claim = outcome.json();
        assert_eq!(
            claim["pv"]["id"],
            "qa/pvc-0a1b2c3d-1111-4000-8000-000000000001"
        );
        assert!(
            claim["pv"]["disk"].as_str().unwrap().ends_with(
                "/providers/Microsoft.Compute/disks/pvc-0a1b2c3d-1111-4000-8000-000000000001"
            ),
            "{claim}"
        );
        assert_eq!(claim["class"]["binding_mode"], "WaitForFirstConsumer");
        assert_eq!(claim["attached_to"], "qa/aks-np1-vmss000000");
        assert_eq!(claim["used_by"], json!(["qa/dev/orders-api-7d9f5b-abc12"]));
        assert_eq!(
            claim["conditions"][0]["type"], "FileSystemResizePending",
            "{claim}"
        );
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);

        let share = run(&["k8s", "pvc", "get", "reports-share", "--fields", "pv"]).json();
        assert_eq!(
            share["pv"]["share"],
            json!({"resource_group": "mc_contoso-qa", "account": "fcontosoqa", "name": "pvc-reports"})
        );
    }

    #[test]
    fn a_disk_held_by_another_node_shows_the_multi_attach_and_names_that_node() {
        let outcome = k8s(&["k8s", "pvc", "get", "qa/uat/orders-data"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let claim = outcome.json();
        assert_eq!(claim["attached_to"], "qa/aks-np1-vmss000001");
        assert_eq!(claim["used_by"], json!(["qa/uat/orders-api-8e0a6c-uat01"]));
        assert_eq!(claim["events"][0]["reason"], "FailedAttachVolume");
        assert!(
            claim["events"][0]["message"]
                .as_str()
                .unwrap()
                .starts_with("Multi-Attach error for volume"),
            "{claim}"
        );
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli k8s node get qa/aks-np1-vmss000001]"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_pending_claim_whose_class_is_missing_names_the_classes() {
        let outcome = run(&["k8s", "pvc", "get", "scratch"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let claim = outcome.json();
        assert_eq!(claim["status"], "Pending");
        assert_eq!(claim["storage_class"], "premium-zrs");
        assert!(claim.get("class").is_none(), "{claim}");
        assert_eq!(claim["events"][0]["reason"], "ProvisioningFailed");
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli k8s storageclass list --cluster qa]"),
            "{}",
            outcome.stderr
        );

        let outcome = run(&["k8s", "pvc", "get", "gone"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("hint: agent-cli k8s pvc list"),
            "{}",
            outcome.stderr
        );
    }
}
