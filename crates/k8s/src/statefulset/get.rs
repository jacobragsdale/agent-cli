use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, items, non_empty, owned_by};
use crate::pod::first_unready;
use crate::pv::access_modes;
use crate::shape::{EventRow, events_in};

use super::{StatefulSetRow, row, templates, wanted};

#[derive(clap::Args)]
pub struct StatefulsetGetArgs {
    /// The statefulset: its id (cluster/namespace/name), namespace/name, or name
    statefulset: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StatefulSetDetail {
    #[serde(flatten)]
    row: StatefulSetRow,
    /// The headless service that names its pods.
    service: Option<String>,
    /// RollingUpdate or OnDelete.
    update_strategy: Option<String>,
    /// Ordinals below it keep the current revision during a rolling update.
    partition: Option<i64>,
    /// The revision its pods run, and the one they are moving to; equal once
    /// a rollout is done.
    current_revision: Option<String>,
    update_revision: Option<String>,
    /// Pods on the update revision, of pods wanted.
    updated: String,
    /// Each claim template and the claims its pods hold.
    claims: Vec<Claims>,
    /// The five newest events about it (SuccessfulCreate, FailedCreate…),
    /// newest first.
    events: Vec<EventRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Claims {
    template: String,
    storage_class: Option<String>,
    requested: Option<String>,
    /// RWO, ROX, RWX, RWOP.
    access_modes: Vec<String>,
    /// `TEMPLATE-STATEFULSET-ORDINAL` for each pod: the PVC ids `pvc get`
    /// takes.
    pvcs: Vec<String>,
}

fn statefulset_get(ctx: &Ctx, args: StatefulsetGetArgs) -> Result<StatefulSetDetail> {
    let (target, name) = args.at.named(ctx, &args.statefulset)?;
    let item = target.json(ctx, &["get", "statefulset", &name, "-o", "json"])?;
    let listed = target.json(ctx, &["get", "pods,events", "-o", "json"])?;
    let pods: Vec<&Value> = items(&listed)
        .filter(|held| held["kind"] == "Pod" && owned_by(held, "StatefulSet", &name))
        .collect();
    let status = &item["status"];
    if status["readyReplicas"].as_i64().unwrap_or(0) < wanted(&item)
        && let Some(next) = first_unready(&target, &pods)
    {
        ctx.note(format!("[next: {next}]"));
    }
    let spec = &item["spec"];
    let namespace = target.namespace.as_deref().unwrap_or_default();
    let claims = templates(&item)
        .filter_map(|claim| {
            let template = non_empty(&claim["metadata"]["name"])?;
            Some(Claims {
                template: template.to_owned(),
                storage_class: non_empty(&claim["spec"]["storageClassName"]).map(str::to_owned),
                requested: non_empty(&claim["spec"]["resources"]["requests"]["storage"])
                    .map(str::to_owned),
                access_modes: access_modes(&claim["spec"]["accessModes"]),
                pvcs: (0..wanted(&item))
                    .map(|ordinal| {
                        format!("{}/{namespace}/{template}-{name}-{ordinal}", target.scope)
                    })
                    .collect(),
            })
        })
        .collect();
    let row = row(&target, &item)
        .ok_or_else(|| anyhow::anyhow!("kubectl answered a statefulset with no name"))?;
    Ok(StatefulSetDetail {
        row,
        service: non_empty(&spec["serviceName"]).map(str::to_owned),
        update_strategy: non_empty(&spec["updateStrategy"]["type"]).map(str::to_owned),
        partition: spec["updateStrategy"]["rollingUpdate"]["partition"].as_i64(),
        current_revision: non_empty(&status["currentRevision"]).map(str::to_owned),
        update_revision: non_empty(&status["updateRevision"]).map(str::to_owned),
        updated: format!(
            "{}/{}",
            status["updatedReplicas"].as_i64().unwrap_or(0),
            wanted(&item)
        ),
        claims,
        events: events_in(&target, &listed, &item),
    })
}

command! {
    pub STATEFULSET_GET = ["k8s", "statefulset", "get"], Read,
    "Describe a statefulset: revisions, partition, each pod's volume claim, events",
    keywords: ["sts", "stateful", "pvc", "claims", "unready"],
    example: "k8s statefulset get prod/web/orders-db --fields ready,claims,events",
    run: statefulset_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn statefulset_get_lists_each_pods_claim_and_its_revisions() {
        let outcome = run(&["k8s", "statefulset", "get", "redis"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["claims"],
            json!([{"template": "data", "storage_class": "managed-csi", "requested": "8Gi",
                "access_modes": ["RWO"], "pvcs": ["qa/dev/data-redis-0"]}])
        );
        assert_eq!(got["current_revision"], "redis-5f6d7c8b9");
        assert_eq!(got["update_revision"], "redis-5f6d7c8b9");
        assert_eq!(got["partition"], 0);
        assert_eq!(got["updated"], "1/1");
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);

        let outcome = run(&["k8s", "statefulset", "get", "gone"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli k8s statefulset list"),
            "{}",
            outcome.stderr
        );
    }
}
