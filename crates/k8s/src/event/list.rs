use agent_cli_core::{Ctx, When, command};
use anyhow::Result;

use crate::kubectl::{At, items, limited};
use crate::shape::{EventRow, event_row};

#[derive(clap::Args)]
pub struct EventListArgs {
    #[command(flatten)]
    at: At,
    /// Only events about this pod: its id, namespace/name or name
    #[arg(long)]
    pod: Option<String>,
    /// Only events last seen after this
    #[arg(long)]
    since: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn event_list(ctx: &Ctx, args: EventListArgs) -> Result<Vec<EventRow>> {
    // A pod's id names its namespace; a bare name keeps the listing's.
    let (target, pod) = match args.pod.as_deref() {
        Some(raw) if raw.contains('/') => {
            let (target, pod) = args.at.named(ctx, raw)?;
            (target, Some(pod))
        }
        other => (args.at.listing(ctx)?, other.map(str::to_owned)),
    };
    let listed = target.json(ctx, &["get", "events", "-o", "json"])?;
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

command! {
    pub EVENT_LIST = ["k8s", "event", "list"], Read,
    "List Kubernetes events, newest first: warnings, back-offs, failed pulls",
    keywords: ["warning", "warnings", "backoff", "crashloop", "why", "failed", "pull", "scheduling", "oomkilled"],
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
}
