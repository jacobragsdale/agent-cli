use agent_cli_core::{Ctx, Failure, When, command, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Dd, cut, epoch, pod_ref, strings, text};

use super::MESSAGE_MAX;

/// A monitor id as an agent was handed it: `4711` or its web URL.
fn monitor_id(dd: &Dd, raw: &str) -> Result<i64> {
    let id = dd.web_id(raw, "monitor", "monitors")?;
    id.parse().map_err(|_| {
        Failure::usage(format!("{raw:?} is not a monitor id"))
            .hint("list them: agent-cli dd monitor list --fields id,name")
            .into()
    })
}

#[derive(clap::Args)]
pub struct MonitorGetArgs {
    /// The monitor: its id (4711) or its https://app.<site>/monitors/4711 URL
    id: String,
    /// Every group, not only those that are not OK
    #[arg(long)]
    all_groups: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MonitorDetail {
    id: i64,
    name: String,
    state: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    priority: Option<i64>,
    query: Option<String>,
    /// Cut to 500 characters.
    message: Option<String>,
    tags: Vec<String>,
    /// When a group last went to Alert.
    triggered: Option<String>,
    /// The groups that are not OK (every group with --all-groups).
    groups: Vec<GroupRow>,
    /// Downtimes muting it now; `dd downtime list --monitor ID` gives the ids.
    downtimes: Vec<Muted>,
    url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GroupRow {
    /// The group's tags: `kube_namespace:web,pod_name:api-1`.
    name: String,
    state: Option<String>,
    triggered: Option<String>,
    resolved: Option<String>,
    /// The k8s pod id (cluster/namespace/pod), when the group names a pod.
    #[serde(skip_serializing_if = "Option::is_none")]
    pod: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Muted {
    scope: Vec<String>,
    start: Option<String>,
    /// Empty: until canceled.
    end: Option<String>,
}

fn monitor_get(ctx: &Ctx, args: MonitorGetArgs) -> Result<MonitorDetail> {
    let dd = Dd::load(ctx)?;
    let id = monitor_id(&dd, &args.id)?;
    let monitor = dd.get(
        ctx,
        &format!("/api/v1/monitor/{id}"),
        &[
            ("group_states", "all".to_owned()),
            ("with_downtimes", "true".to_owned()),
        ],
    )?;
    let mut groups: Vec<GroupRow> = monitor["state"]["groups"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, group)| {
            let name = text(&group["name"]).unwrap_or_else(|| key.clone());
            let tags = Value::from(name.split(',').map(str::to_owned).collect::<Vec<_>>());
            GroupRow {
                pod: pod_ref(&tags),
                state: text(&group["status"]),
                triggered: group["last_triggered_ts"]
                    .as_i64()
                    .filter(|ts| *ts > 0)
                    .and_then(epoch),
                resolved: group["last_resolved_ts"]
                    .as_i64()
                    .filter(|ts| *ts > 0)
                    .and_then(epoch),
                name,
            }
        })
        .collect();
    let triggered = groups
        .iter()
        .filter_map(|group| group.triggered.clone())
        .max();
    groups.sort_by(|a, b| {
        let ok = |group: &GroupRow| group.state.as_deref() == Some("OK");
        ok(a).cmp(&ok(b)).then(b.triggered.cmp(&a.triggered))
    });
    let total = groups.len();
    if !args.all_groups {
        groups.retain(|group| group.state.as_deref() != Some("OK"));
        if groups.len() < total {
            ctx.note(format!(
                "[{} OK groups not shown; --all-groups shows them]",
                total - groups.len()
            ));
        }
    }
    // An alert on a pod is answered by that pod's errors from just before it.
    let alerting = groups
        .iter()
        .find(|group| group.state.as_deref() == Some("Alert"))
        .filter(|_| monitor["overall_state"] == "Alert");
    if let Some((pod, triggered)) = alerting.and_then(|group| {
        Some((
            group.pod.as_deref()?,
            group.triggered.as_deref()?.parse::<When>().ok()?,
        ))
    }) {
        let since = utc_time(triggered.0 - time::Duration::minutes(15));
        ctx.note(format!(
            "[next: agent-cli dd log list --pod {pod} --status error --since {since}]"
        ));
    }
    let downtimes = monitor["matching_downtimes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|downtime| downtime["active"] != false)
        .map(|downtime| Muted {
            scope: match &downtime["scope"] {
                Value::String(scope) => vec![scope.clone()],
                other => strings(other),
            },
            start: downtime["start"].as_i64().and_then(epoch),
            end: downtime["end"].as_i64().and_then(epoch),
        })
        .collect();
    Ok(MonitorDetail {
        id,
        name: text(&monitor["name"]).unwrap_or_default(),
        state: text(&monitor["overall_state"]),
        kind: text(&monitor["type"]),
        priority: monitor["priority"].as_i64(),
        query: text(&monitor["query"]),
        message: text(&monitor["message"]).map(|message| cut(&message, MESSAGE_MAX)),
        tags: strings(&monitor["tags"]),
        triggered,
        groups,
        downtimes,
        url: dd.app(&format!("/monitors/{id}")),
    })
}

command! {
    pub MONITOR_GET = ["dd", "monitor", "get"], Read,
    "Show a Datadog monitor: its query, state, the groups that triggered, downtimes",
    keywords: ["why", "groups", "threshold", "query", "which pods", "triggered"],
    example: "dd monitor get 4711 --fields state,triggered,groups",
    run: monitor_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testing::dd;

    fn monitor_detail() -> Value {
        json!({
            "id": 4711,
            "name": "[prod] api error rate above 5%",
            "type": "query alert",
            "query": "sum(last_5m):sum:trace.http.request.errors{service:api,env:prod}.as_count() / sum:trace.http.request.hits{service:api,env:prod}.as_count() > 0.05",
            "message": "api is failing requests. @slack-contoso-web",
            "tags": ["service:api", "env:prod", "team:web"],
            "priority": 2,
            "overall_state": "Alert",
            "overall_state_modified": "2026-09-28T21:36:00+00:00",
            "multi": true,
            "state": {"groups": {
                "kube_cluster_name:prod,kube_namespace:web,pod_name:api-7d9f8c6b5-x2k4q": {
                    "name": "kube_cluster_name:prod,kube_namespace:web,pod_name:api-7d9f8c6b5-x2k4q",
                    "status": "Alert", "last_triggered_ts": 1_790_631_360, "last_resolved_ts": 0
                },
                "kube_cluster_name:prod,kube_namespace:web,pod_name:api-7d9f8c6b5-m3n5p": {
                    "name": "kube_cluster_name:prod,kube_namespace:web,pod_name:api-7d9f8c6b5-m3n5p",
                    "status": "OK", "last_triggered_ts": 1_790_631_300, "last_resolved_ts": 1_790_632_680
                }
            }},
            "matching_downtimes": [{"id": 2_942_947_856_i64, "active": true, "monitor_id": 4711, "start": 1_790_682_000, "end": 1_790_683_800, "scope": ["*"]}]
        })
    }

    #[test]
    fn monitor_get_takes_an_id_or_its_url_and_shows_the_alerting_groups_with_pod_ids() {
        for id in [
            "4711",
            "https://app.datadoghq.eu/monitors/4711?from_ts=1&to_ts=2",
        ] {
            let (outcome, transport) = dd(
                &["dd", "monitor", "get", id],
                vec![Answer::json(&monitor_detail())],
            );
            assert_eq!(outcome.code, 0, "{outcome:?}");
            let got = outcome.json();
            assert_eq!(got["state"], "Alert");
            assert_eq!(got["triggered"], "2026-09-28T21:36:00Z");
            assert_eq!(
                got["groups"].as_array().unwrap().len(),
                1,
                "OK groups are left out"
            );
            assert_eq!(got["groups"][0]["pod"], "prod/web/api-7d9f8c6b5-x2k4q");
            assert_eq!(got["groups"][0]["resolved"], Value::Null);
            assert_eq!(got["downtimes"][0]["end"], "2026-09-29T12:10:00Z");
            assert_eq!(got["url"], "https://app.datadoghq.eu/monitors/4711");
            assert!(
                outcome
                    .stderr
                    .contains("[1 OK groups not shown; --all-groups shows them]")
            );
            assert!(
                outcome.stderr.contains(
                    "[next: agent-cli dd log list --pod prod/web/api-7d9f8c6b5-x2k4q --status error --since 2026-09-28T21:21:00Z]"
                ),
                "{}",
                outcome.stderr
            );
            assert_eq!(
                transport.sent()[0].url,
                "https://api.datadoghq.eu/api/v1/monitor/4711?group_states=all&with_downtimes=true"
            );
        }
        let (outcome, _) = dd(
            &["dd", "monitor", "get", "4711", "--all-groups"],
            vec![Answer::json(&monitor_detail())],
        );
        assert_eq!(
            outcome.json()["groups"][1]["resolved"],
            "2026-09-28T21:58:00Z"
        );
        let mut ok = monitor_detail();
        ok["overall_state"] = json!("OK");
        let (outcome, _) = dd(&["dd", "monitor", "get", "4711"], vec![Answer::json(&ok)]);
        assert!(
            !outcome.stderr.contains("[next:"),
            "only an alert names a next step: {}",
            outcome.stderr
        );

        let (outcome, transport) = dd(
            &[
                "dd",
                "monitor",
                "get",
                "https://app.datadoghq.com/monitors/4711",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "another site's URL: {outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("the configured Datadog site is datadoghq.eu"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().is_empty());
        let (outcome, _) = dd(&["dd", "monitor", "get", "api-errors"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
