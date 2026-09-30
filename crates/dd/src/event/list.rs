use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Dd, Scope, Window, cut, limited, pod_ref, search_query, text};
use crate::search::{MESSAGE_MAX, PAGE_MAX};

#[derive(clap::Args)]
pub struct EventListArgs {
    /// Datadog event search syntax: 'source:kubernetes', 'tags:deployment'
    query: Option<String>,
    /// From when (default 1h before --until)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
    #[command(flatten)]
    scope: Scope,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct EventRow {
    /// The event's id in Datadog.
    id: String,
    time: Option<String>,
    title: Option<String>,
    /// Where it came from: kubernetes, a monitor alert, a deploy tool.
    source: Option<String>,
    /// Cut to 300 characters.
    message: Option<String>,
    service: Option<String>,
    /// The k8s pod id (cluster/namespace/pod), when the event names one.
    pod: Option<String>,
}

fn event_list(ctx: &Ctx, args: EventListArgs) -> Result<Vec<EventRow>> {
    let dd = Dd::load(ctx)?;
    let window = Window::new(ctx, args.since, args.until, "1h")?;
    let terms = args.scope.tags(&dd, false)?;
    let query = [
        ("filter[query]", search_query(args.query.as_deref(), &terms)),
        ("filter[from]", window.since_ms()),
        ("filter[to]", window.until_ms()),
        ("sort", "-timestamp".to_owned()),
        ("page[limit]", args.limit.min(PAGE_MAX).to_string()),
    ];
    let found = dd.get(ctx, "/api/v2/events", &query)?;
    let rows: Vec<EventRow> = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let attributes = &item["attributes"];
            let inner = &attributes["attributes"];
            let tags = &attributes["tags"];
            EventRow {
                id: text(&item["id"]).unwrap_or_default(),
                time: text(&attributes["timestamp"]).map(|time| agent_cli_core::utc(&time)),
                title: text(&inner["title"]).or_else(|| text(&attributes["title"])),
                source: text(&inner["source_type_name"])
                    .or_else(|| text(&inner["evt"]["source"]))
                    .or_else(|| crate::client::tag(tags, "source").map(str::to_owned)),
                message: text(&attributes["message"]).map(|text| cut(&text, MESSAGE_MAX)),
                service: text(&inner["service"])
                    .or_else(|| crate::client::tag(tags, "service").map(str::to_owned)),
                pod: pod_ref(tags),
            }
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub EVENT_LIST = ["dd", "event", "list"], Read,
    "List Datadog events: deploys, monitor alerts, kubernetes events kept past 1h",
    keywords: ["deploy", "change", "history", "happened", "timeline", "datadog events"],
    example: "dd event list 'source:kubernetes' --namespace web --since 2h --fields time,title,pod",
    run: event_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::dd;

    #[test]
    fn event_list_reads_the_v2_event_stream() {
        let (outcome, transport) = dd(
            &[
                "dd",
                "event",
                "list",
                "source:kubernetes",
                "--pod",
                "prod/web/worker-5c4d3e9f1-q8zt1",
                "--since",
                "2026-09-29T11:00:00Z",
                "--until",
                "2026-09-29T12:00:00Z",
            ],
            vec![Answer::json(&json!({
                "data": [{
                    "id": "AAAAAZIxcNZ7ZOvxZQAAAABBWkl4Y05aN0FBQ21ERlpZT2tGS0FBQUE",
                    "type": "event",
                    "attributes": {
                        "timestamp": "2026-09-29T11:52:30.000Z",
                        "message": "Back-off restarting failed container worker in pod worker-5c4d3e9f1-q8zt1_web",
                        "tags": ["source:kubernetes", "kube_cluster_name:prod", "kube_namespace:web", "pod_name:worker-5c4d3e9f1-q8zt1"],
                        "attributes": {"title": "Events from the Pod web/worker-5c4d3e9f1-q8zt1", "source_type_name": "kubernetes"}
                    }
                }],
                "meta": {"page": {}}
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = &outcome.json()[0];
        assert_eq!(row["time"], "2026-09-29T11:52:30Z");
        assert_eq!(row["source"], "kubernetes");
        assert_eq!(row["pod"], "prod/web/worker-5c4d3e9f1-q8zt1");
        let url = &transport.sent()[0].url;
        assert!(url.starts_with("https://api.datadoghq.eu/api/v2/events?filter%5Bquery%5D=source%3Akubernetes+kube_cluster_name%3Aprod+kube_namespace%3Aweb+pod_name%3Aworker-5c4d3e9f1-q8zt1&filter%5Bfrom%5D=1790679600000&"), "{url}");
    }
}
