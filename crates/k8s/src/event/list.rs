use agent_cli_core::{Ctx, Failure, When, command};
use anyhow::Result;
use serde_json::Value;

use crate::kind::kind_named;
use crate::kubectl::{At, Target, items, limited};
use crate::shape::{EventRow, event_row};

#[derive(clap::Args)]
pub struct EventListArgs {
    #[command(flatten)]
    at: At,
    /// Only events about this pod: its id, namespace/name or name
    #[arg(long, conflicts_with = "object")]
    pod: Option<String>,
    /// Only events about this object, as KIND/NAME (pvc/data-orders-db-0,
    /// node/aks-user-27182818-vmss000001); NAME may be its id
    #[arg(long)]
    object: Option<String>,
    /// Only events last seen after this
    #[arg(long)]
    since: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn event_list(ctx: &Ctx, args: EventListArgs) -> Result<Vec<EventRow>> {
    // A pod's id names its namespace; a bare name keeps the listing's.
    let (target, pod, listed) = match (&args.object, args.pod.as_deref()) {
        (Some(object), _) => {
            let (target, listed) = about(ctx, &args.at, object)?;
            (target, None, listed)
        }
        (None, Some(raw)) if raw.contains('/') => {
            let (target, pod) = args.at.named(ctx, raw)?;
            let listed = target.json(ctx, &["get", "events", "-o", "json"])?;
            (target, Some(pod), listed)
        }
        (None, other) => {
            let target = args.at.listing(ctx)?;
            let listed = target.json(ctx, &["get", "events", "-o", "json"])?;
            (target, other.map(str::to_owned), listed)
        }
    };
    let mut rows: Vec<_> = items(&listed)
        .filter(|item| {
            pod.as_deref()
                .is_none_or(|pod| item["involvedObject"]["name"].as_str() == Some(pod))
        })
        .map(|item| event_row(&target, item))
        .collect();
    if let Some(since) = args.since {
        rows.retain(|(last, _)| last.is_some_and(|last| last >= since.0));
    }
    rows.sort_by_key(|(last, _)| std::cmp::Reverse(*last));
    Ok(limited(
        ctx,
        rows.into_iter().map(|(_, row)| row).collect(),
        args.limit,
    ))
}

/// The events about one object of any kind, by kind and name. Whether the
/// kind is namespaced decides how NAME reads; a cluster-scoped object's
/// events sit in `default` or wherever its controller wrote them, so they
/// are read across namespaces.
fn about(ctx: &Ctx, at: &At, object: &str) -> Result<(Target, Value)> {
    let Some((word, raw)) = object.split_once('/') else {
        return Err(Failure::usage(format!(
            "--object {object:?} is not KIND/NAME (pvc/data-orders-db-0)"
        ))
        .into());
    };
    let kind = kind_named(ctx, &at.cluster_of(ctx, raw)?, word)?;
    let (target, name) = if kind.namespaced {
        at.named(ctx, raw)?
    } else {
        at.named_cluster(ctx, raw)?
    };
    let selector = format!(
        "involvedObject.kind={},involvedObject.name={name}",
        kind.kind
    );
    let mut argv = vec!["get", "events", "-o", "json", "--field-selector", &selector];
    if target.cluster_scoped {
        argv.push("--all-namespaces");
    }
    let listed = target.json(ctx, &argv)?;
    Ok((target, listed))
}

command! {
    pub EVENT_LIST = ["k8s", "event", "list"], Read,
    "List Kubernetes events, newest first: warnings, back-offs, failed pulls",
    keywords: ["warning", "warnings", "backoff", "crashloop", "why", "failed", "pull", "scheduling", "oomkilled", "notready", "involved"],
    example: "k8s event list --pod qa/dev/orders-worker-5c4d3e-q8zt --since 1h --fields type,reason,message,last_seen",
    run: event_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{k8s, run};

    #[test]
    fn events_come_newest_first_and_narrow_to_one_pod() {
        let outcome = run(&["k8s", "event", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["reason"], "BackOff");
        assert_eq!(rows[0]["type"], "Warning");
        assert_eq!(rows[0]["count"], 17);
        assert_eq!(rows[0]["object"], "Pod/orders-worker-5c4d3e-q8zt");
        assert_eq!(rows[0]["last_seen"], "2026-09-12T12:30:00Z");
        assert_eq!(rows[2]["reason"], "ScalingReplicaSet");

        let outcome = run(&[
            "k8s",
            "event",
            "list",
            "--pod",
            "orders-api-7d9f5b-abc12",
            "--fields",
            "reason",
        ]);
        assert_eq!(outcome.json(), json!([{"reason": "Pulled"}]));
    }

    #[test]
    fn events_narrow_to_a_pod_by_id_and_to_a_window() {
        let outcome = k8s(&[
            "k8s",
            "event",
            "list",
            "--pod",
            "qa/dev/orders-worker-5c4d3e-q8zt",
            "--fields",
            "reason,last_seen",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"reason": "BackOff", "last_seen": "2026-09-12T12:30:00Z"}])
        );
        let recent = run(&[
            "k8s",
            "event",
            "list",
            "--since",
            "2026-09-12T11:00:00Z",
            "--fields",
            "reason",
        ]);
        assert_eq!(
            recent.json(),
            json!([{"reason": "BackOff"}, {"reason": "Pulled"}])
        );
    }

    #[test]
    fn events_narrow_to_any_object_by_kind_a_cluster_scoped_one_across_namespaces() {
        let outcome = run(&[
            "k8s",
            "event",
            "list",
            "--object",
            "pvc/scratch",
            "--fields",
            "reason,object",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"reason": "ProvisioningFailed", "object": "PersistentVolumeClaim/scratch"}])
        );

        let outcome = k8s(&[
            "k8s",
            "event",
            "list",
            "--object",
            "node/qa/aks-np1-vmss000001",
            "--fields",
            "reason,namespace",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"reason": "NodeNotReady", "namespace": "default"}])
        );

        let outcome = run(&["k8s", "event", "list", "--object", "scratch"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        let outcome = run(&[
            "k8s",
            "event",
            "list",
            "--object",
            "pvc/scratch",
            "--pod",
            "redis-0",
        ]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
