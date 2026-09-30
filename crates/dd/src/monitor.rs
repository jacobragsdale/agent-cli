//! Monitors and downtimes: what is alerting and why, and muting it for a
//! bounded time. A mute is Destructive: it can be undone, a missed page
//! cannot, so it needs `--yes` and never lasts more than 7 days.

use std::time::Duration;

use agent_cli_core::{Ctx, Effect, Failure, Method, Span, command, now, utc, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Dd, cut, epoch, limited, pod_ref, strings, text};

const MESSAGE_MAX: usize = 500;
/// The longest mute: long enough for a weekend, short enough that a
/// forgotten one ends by itself.
const MUTE_MAX: Duration = Duration::from_secs(7 * 86_400);
/// Datadog's monitor states, as its search syntax spells them.
const STATES: &[&str] = &[
    "Alert", "Warn", "No Data", "OK", "Ignored", "Skipped", "Unknown",
];

/// A state as typed (`alert`, `no data`, `NoData`) in Datadog's spelling.
fn state(raw: &str) -> Result<&'static str> {
    let squashed: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    STATES
        .iter()
        .find(|state| state.replace(' ', "").to_ascii_lowercase() == squashed)
        .copied()
        .ok_or_else(|| {
            Failure::usage(format!(
                "{raw:?} is not a monitor state; --state takes {}",
                STATES.join(", ")
            ))
            .into()
        })
}

/// A monitor id as an agent was handed it: `4711` or its web URL.
fn monitor_id(dd: &Dd, raw: &str) -> Result<i64> {
    let id = dd.web_id(raw, "monitor", "monitors")?;
    id.parse().map_err(|_| {
        Failure::usage(format!("{raw:?} is not a monitor id"))
            .hint("list them: agent-cli dd monitor list --fields id,name")
            .into()
    })
}

// ---------- dd monitor list ----------

#[derive(clap::Args)]
pub struct MonitorListArgs {
    /// Monitor search syntax: 'service:api', 'type:metric', 'title:"error rate"'
    query: Option<String>,
    /// Alert, Warn, "No Data", OK (repeatable; any case)
    #[arg(long, value_delimiter = ',')]
    state: Vec<String>,
    /// Monitor tags: team:web (repeatable, all must hold)
    #[arg(long)]
    tag: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MonitorRow {
    /// What `dd monitor get` takes.
    id: i64,
    name: String,
    /// Alert, Warn, No Data, OK.
    state: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    priority: Option<i64>,
    /// When it last went to Alert.
    triggered: Option<String>,
    tags: Vec<String>,
}

fn monitor_list(ctx: &Ctx, args: MonitorListArgs) -> Result<Vec<MonitorRow>> {
    let states = args
        .state
        .iter()
        .map(|raw| {
            state(raw).map(|state| {
                if state.contains(' ') {
                    format!("\"{state}\"")
                } else {
                    state.to_owned()
                }
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let dd = Dd::load(ctx)?;
    let mut terms: Vec<String> = crate::client::any_of("status", &states)
        .into_iter()
        .collect();
    terms.extend(args.tag.iter().map(|tag| format!("tag:\"{tag}\"")));
    let search = crate::client::search_query(args.query.as_deref(), &terms);
    let mut query = vec![
        ("per_page", args.limit.min(1000).to_string()),
        ("page", "0".to_owned()),
    ];
    if search != "*" {
        query.insert(0, ("query", search));
    }
    let found = dd.get(ctx, "/api/v1/monitor/search", &query)?;
    let rows: Vec<MonitorRow> = found["monitors"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|monitor| MonitorRow {
            id: monitor["id"].as_i64().unwrap_or_default(),
            name: text(&monitor["name"]).unwrap_or_default(),
            state: text(&monitor["status"]),
            kind: text(&monitor["type"]),
            priority: monitor["priority"].as_i64(),
            triggered: monitor["last_triggered_ts"].as_i64().and_then(epoch),
            tags: strings(&monitor["tags"]),
        })
        .collect();
    if let Some(total) = found["metadata"]["total_count"].as_u64()
        && total > rows.len() as u64
    {
        ctx.note(format!("[{} of {total}; --limit N]", rows.len()));
    }
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub MONITOR_LIST = ["dd", "monitor", "list"], Read,
    "List Datadog monitors alerting or not, by state or tag, with when each triggered",
    keywords: ["alert", "alarm", "firing", "alerting", "paging", "team", "now"],
    example: "dd monitor list --state Alert,Warn --tag team:web --fields id,name,state,triggered",
    run: monitor_list,
}

// ---------- dd monitor get ----------

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

// ---------- dd downtime list ----------

#[derive(clap::Args)]
pub struct DowntimeListArgs {
    /// Only downtimes that name this monitor id
    #[arg(long)]
    monitor: Option<i64>,
    /// Include ended and canceled downtimes
    #[arg(long)]
    all: bool,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DowntimeRow {
    /// What `dd downtime cancel` takes.
    id: String,
    /// active, scheduled, ended, canceled.
    status: Option<String>,
    monitor: Option<i64>,
    /// Or every monitor with these tags.
    monitor_tags: Vec<String>,
    scope: Option<String>,
    start: Option<String>,
    /// Empty: until canceled.
    end: Option<String>,
    message: Option<String>,
}

fn downtime_row(item: &Value) -> DowntimeRow {
    let attributes = &item["attributes"];
    let schedule = &attributes["schedule"];
    let current = if schedule["current_downtime"].is_object() {
        &schedule["current_downtime"]
    } else {
        schedule
    };
    let identifier = &attributes["monitor_identifier"];
    DowntimeRow {
        id: text(&item["id"]).unwrap_or_default(),
        status: text(&attributes["status"]),
        monitor: identifier["monitor_id"].as_i64(),
        monitor_tags: strings(&identifier["monitor_tags"]),
        scope: text(&attributes["scope"]),
        start: text(&current["start"]).map(|at| utc(&at)),
        end: text(&current["end"]).map(|at| utc(&at)),
        message: text(&attributes["message"]).map(|message| cut(&message, MESSAGE_MAX)),
    }
}

fn downtime_list(ctx: &Ctx, args: DowntimeListArgs) -> Result<Vec<DowntimeRow>> {
    let dd = Dd::load(ctx)?;
    // A filter on the monitor is applied here, so it reads a whole page.
    let page = if args.monitor.is_some() {
        1000
    } else {
        args.limit.min(1000)
    };
    let found = dd.get(
        ctx,
        "/api/v2/downtime",
        &[
            ("current_only", (!args.all).to_string()),
            ("page[limit]", page.to_string()),
        ],
    )?;
    let rows = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(downtime_row)
        .filter(|row| {
            args.monitor
                .is_none_or(|monitor| row.monitor == Some(monitor))
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub DOWNTIME_LIST = ["dd", "downtime", "list"], Read,
    "List Datadog downtimes: which monitors are muted, for what scope and until when",
    keywords: ["muted", "silenced", "maintenance", "scheduled", "snoozed", "active", "current"],
    example: "dd downtime list --monitor 4711 --fields id,status,end",
    run: downtime_list,
}

// ---------- dd downtime create ----------

#[derive(clap::Args)]
pub struct DowntimeCreateArgs {
    /// The monitor to mute
    #[arg(
        long,
        required_unless_present = "monitor_tag",
        conflicts_with = "monitor_tag"
    )]
    monitor: Option<i64>,
    /// Mute every monitor with these tags instead (repeatable)
    #[arg(long, value_delimiter = ',')]
    monitor_tag: Vec<String>,
    /// How long, at most 7d: 30m, 2h, 1d
    #[arg(long = "for", required = true)]
    length: Span,
    /// The groups to mute: '*' (every group), 'pod_name:api-1', 'env:prod'
    #[arg(long, default_value = "*")]
    scope: String,
    /// Why, as Datadog shows it
    #[arg(long)]
    message: Option<String>,
}

fn downtime_create(ctx: &Ctx, args: DowntimeCreateArgs) -> Result<DowntimeRow> {
    let length = args.length.0;
    if length.is_zero() || length > MUTE_MAX {
        return Err(Failure::usage(format!(
            "--for is {}s; a mute lasts more than 0 and at most 7d",
            length.as_secs()
        ))
        .hint("mute again when it ends, if it still needs it")
        .into());
    }
    let dd = Dd::load(ctx)?;
    let end = now() + length;
    let identifier = match args.monitor {
        Some(monitor) => json!({"monitor_id": monitor}),
        None => json!({"monitor_tags": args.monitor_tag}),
    };
    let mut attributes = json!({
        "monitor_identifier": identifier,
        "scope": args.scope,
        "schedule": {"end": utc_time(end)},
        "display_timezone": "UTC",
    });
    if let Some(message) = args.message {
        attributes["message"] = json!(message);
    }
    let created = dd.change(
        ctx,
        Effect::Destructive,
        Method::Post,
        "/api/v2/downtime",
        Some(json!({"data": {"type": "downtime", "attributes": attributes}})),
    )?;
    let row = downtime_row(&created["data"]);
    ctx.note(format!(
        "[unmute early with agent-cli dd downtime cancel {}]",
        row.id
    ));
    Ok(row)
}

command! {
    pub DOWNTIME_CREATE = ["dd", "downtime", "create"], Destructive,
    "Mute a Datadog monitor for a bounded time (--for, at most 7 days)",
    keywords: ["mute", "silence", "snooze", "suppress", "quiet", "pause", "during", "deploy", "minutes"],
    example: "dd downtime create --monitor 4711 --for 30m --message 'deploy v1.4.2' --dry-run",
    run: downtime_create,
}

// ---------- dd downtime cancel ----------

#[derive(clap::Args)]
pub struct DowntimeCancelArgs {
    /// The downtime id, from dd downtime list
    id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Canceled {
    id: String,
    canceled: bool,
}

fn downtime_cancel(ctx: &Ctx, args: DowntimeCancelArgs) -> Result<Canceled> {
    let id = args.id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(Failure::usage(format!("{id:?} is not a downtime id"))
            .hint("list them: agent-cli dd downtime list --fields id,monitor,end")
            .into());
    }
    let dd = Dd::load(ctx)?;
    dd.change(
        ctx,
        Effect::Write,
        Method::Delete,
        &format!("/api/v2/downtime/{id}"),
        None,
    )?;
    Ok(Canceled {
        id: id.to_owned(),
        canceled: true,
    })
}

command! {
    pub DOWNTIME_CANCEL = ["dd", "downtime", "cancel"], Write,
    "Unmute a Datadog monitor now, ending its downtime before it runs out",
    keywords: ["unsilence", "resume", "early", "stop muting", "monitor"],
    example: "dd downtime cancel 00000000-0000-4000-8000-00000000d001 --dry-run",
    run: downtime_cancel,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, assert_dry_run};
    use serde_json::json;

    use super::*;
    use crate::testkit::{dd, sent_bodies};

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

    #[test]
    fn monitor_list_searches_by_state_and_tag_in_datadog_syntax() {
        let (outcome, transport) = dd(
            &[
                "dd",
                "monitor",
                "list",
                "--state",
                "alert,no data",
                "--tag",
                "team:web",
                "--limit",
                "1",
            ],
            vec![Answer::json(&json!({
                "monitors": [{
                    "id": 4712, "name": "[prod] worker crash-looping", "status": "Alert", "type": "query alert",
                    "priority": 1, "tags": ["service:worker", "team:web"], "last_triggered_ts": 1_790_631_660,
                    "classification": "metric", "query": "max(last_10m):…"
                }],
                "metadata": {"total_count": 2, "page_count": 2, "page": 0, "per_page": 1},
                "counts": {"status": [{"name": "Alert", "count": 1}]}
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "id": 4712, "name": "[prod] worker crash-looping", "state": "Alert", "type": "query alert",
                "priority": 1, "triggered": "2026-09-28T21:41:00Z", "tags": ["service:worker", "team:web"]
            }])
        );
        assert!(
            outcome.stderr.contains("[1 of 2; --limit N]"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v1/monitor/search?query=status%3A%28Alert+OR+%22No+Data%22%29+tag%3A%22team%3Aweb%22&per_page=1&page=0"
        );
        let (outcome, _) = dd(&["dd", "monitor", "list", "--state", "firing"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Alert, Warn, No Data, OK"),
            "{}",
            outcome.stderr
        );
    }

    fn downtime(id: &str, monitor: i64, end: &str) -> Value {
        json!({
            "id": id, "type": "downtime",
            "attributes": {
                "status": "active", "scope": "*", "message": "deploy v1.4.2", "display_timezone": "UTC",
                "canceled": null, "created": "2026-09-29T11:40:00.300497+00:00",
                "monitor_identifier": {"monitor_id": monitor},
                "schedule": {"start": "2026-09-29T11:40:00.286133+00:00", "end": end}
            },
            "relationships": {"monitor": {"data": {"type": "monitors", "id": monitor.to_string()}}}
        })
    }

    #[test]
    fn downtime_list_filters_by_monitor_and_prints_utc() {
        let (outcome, transport) = dd(
            &["dd", "downtime", "list", "--monitor", "4711"],
            vec![Answer::json(&json!({
                "data": [
                    downtime("00000000-0000-4000-8000-00000000d001", 4711, "2026-09-29T12:10:00+00:00"),
                    downtime("00000000-0000-4000-8000-00000000d002", 4712, "2026-09-29T13:00:00+00:00")
                ],
                "meta": {"page": {"total_filtered_count": 2}}
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], "00000000-0000-4000-8000-00000000d001");
        assert_eq!(rows[0]["start"], "2026-09-29T11:40:00Z");
        assert_eq!(rows[0]["end"], "2026-09-29T12:10:00Z");
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v2/downtime?current_only=true&page%5Blimit%5D=1000"
        );
    }

    #[test]
    fn downtime_create_needs_yes_and_a_for_of_at_most_seven_days() {
        let plan = assert_dry_run(
            &[crate::DOMAIN],
            &[
                "dd",
                "downtime",
                "create",
                "--monitor",
                "4711",
                "--for",
                "30m",
                "--message",
                "deploy v1.4.2",
            ],
            vec![],
        );
        assert_eq!(plan[0]["method"], "POST");
        assert_eq!(plan[0]["url"], "https://api.datadoghq.com/api/v2/downtime");
        let attributes = &plan[0]["body"]["data"]["attributes"];
        assert_eq!(
            attributes["monitor_identifier"],
            json!({"monitor_id": 4711})
        );
        assert_eq!(attributes["scope"], "*");
        assert!(
            attributes["schedule"]["end"]
                .as_str()
                .unwrap()
                .ends_with('Z')
        );

        for length in ["8d", "2w", "0s"] {
            let (outcome, transport) = dd(
                &[
                    "dd",
                    "downtime",
                    "create",
                    "--monitor",
                    "4711",
                    "--for",
                    length,
                    "--yes",
                ],
                vec![],
            );
            assert_eq!(outcome.code, 2, "{length}: {outcome:?}");
            assert!(outcome.stderr.contains("at most 7d"), "{}", outcome.stderr);
            assert!(transport.sent().is_empty());
        }
        let (outcome, transport) = dd(&["dd", "downtime", "create", "--monitor", "4711"], vec![]);
        assert_eq!(outcome.code, 2, "--for is required: {outcome:?}");
        assert!(transport.sent().is_empty());
        let (outcome, transport) = dd(
            &[
                "dd",
                "downtime",
                "create",
                "--monitor",
                "4711",
                "--for",
                "30m",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "destructive without --yes: {outcome:?}");
        assert!(outcome.stderr.contains("--yes"), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn downtime_create_with_yes_mutes_and_says_how_to_unmute() {
        let (outcome, transport) = dd(
            &[
                "dd",
                "downtime",
                "create",
                "--monitor-tag",
                "service:api",
                "--for",
                "1h",
                "--scope",
                "env:prod",
                "--yes",
            ],
            vec![Answer::json(
                &json!({"data": downtime("00000000-0000-4000-8000-00000000d003", 0, "2026-09-29T13:00:00+00:00")}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["id"], "00000000-0000-4000-8000-00000000d003");
        assert!(
            outcome
                .stderr
                .contains("agent-cli dd downtime cancel 00000000-0000-4000-8000-00000000d003]"),
            "{}",
            outcome.stderr
        );
        let body = &sent_bodies(&transport)[0];
        assert_eq!(
            body["data"]["attributes"]["monitor_identifier"],
            json!({"monitor_tags": ["service:api"]})
        );
        assert_eq!(body["data"]["attributes"]["scope"], "env:prod");
        assert_eq!(transport.sent()[0].method, Method::Post);
    }

    #[test]
    fn downtime_cancel_deletes_and_its_dry_run_sends_nothing() {
        let plan = assert_dry_run(
            &[crate::DOMAIN],
            &[
                "dd",
                "downtime",
                "cancel",
                "00000000-0000-4000-8000-00000000d001",
            ],
            vec![],
        );
        assert_eq!(
            plan[0]["url"],
            "https://api.datadoghq.com/api/v2/downtime/00000000-0000-4000-8000-00000000d001"
        );
        let (outcome, transport) = dd(
            &[
                "dd",
                "downtime",
                "cancel",
                "00000000-0000-4000-8000-00000000d001",
            ],
            vec![Answer::status(204, "")],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "00000000-0000-4000-8000-00000000d001", "canceled": true})
        );
        assert_eq!(transport.sent()[0].method, Method::Delete);
        let (outcome, _) = dd(&["dd", "downtime", "cancel", "../monitor/1"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
