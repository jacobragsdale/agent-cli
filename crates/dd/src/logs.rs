//! Logs and events: the searches an agent debugging a service makes first,
//! bounded to compact rows, and the counts that answer "how many" without
//! reading rows at all.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Failure, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Dd, Scope, Window, any_of, cut, limited, pod_ref, search_query, text, utc_ms};

/// A log's status, as Datadog's search syntax spells it.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum LogStatus {
    Emergency,
    Alert,
    Critical,
    Error,
    Warn,
    Notice,
    Info,
    Debug,
    Ok,
}

/// `values` by their command-line names, for a search term.
pub(crate) fn names<T: clap::ValueEnum>(values: &[T]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| Some(value.to_possible_value()?.get_name().to_owned()))
        .collect()
}

/// Longer messages are cut, with the count of what was left out.
const MESSAGE_MAX: usize = 300;
/// One page of the logs and spans search APIs.
pub(crate) const PAGE_MAX: usize = 1000;
/// Searches over a longer window are slow; counting is not.
const SLOW_WINDOW: i64 = 86_400;

// ---------- dd log list ----------

#[derive(clap::Args)]
pub struct LogListArgs {
    /// Datadog log search syntax, ANDed with the flags: '@http.status_code:502 timeout'
    query: Option<String>,
    /// From when (default 15m before --until)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
    /// Log status (repeatable)
    #[arg(long, value_delimiter = ',', ignore_case = true)]
    status: Vec<LogStatus>,
    #[command(flatten)]
    scope: Scope,
    /// Log indexes to search (default: every index)
    #[arg(long, value_delimiter = ',')]
    index: Vec<String>,
    /// Whole messages and every attribute
    #[arg(long)]
    full: bool,
    /// At most 1000 (one page); count more with dd log-count list
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LogRow {
    /// The log's id in Datadog.
    id: String,
    /// UTC, with milliseconds.
    time: String,
    status: Option<String>,
    service: Option<String>,
    host: Option<String>,
    /// Cut to 300 characters unless --full.
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorInfo>,
    trace_id: Option<String>,
    /// The k8s pod id (cluster/namespace/pod) that `agent-cli k8s pod get` takes.
    pod: Option<String>,
    /// Every attribute, with --full.
    #[serde(skip_serializing_if = "Option::is_none")]
    attributes: Option<Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ErrorInfo {
    kind: Option<String>,
    message: Option<String>,
}

fn log_list(ctx: &Ctx, args: LogListArgs) -> Result<Vec<LogRow>> {
    check_page(args.limit, "logs")?;
    let dd = Dd::load(ctx)?;
    let window = Window::new(ctx, args.since, args.until, "15m")?;
    slow_window(ctx, window);
    let mut terms = args.scope.tags(&dd, true)?;
    terms.extend(any_of("status", &names(&args.status)));
    let mut filter = json!({
        "query": search_query(args.query.as_deref(), &terms),
        "from": window.since_ms(),
        "to": window.until_ms(),
    });
    if !args.index.is_empty() {
        filter["indexes"] = json!(args.index);
    }
    let body = json!({"filter": filter, "sort": "-timestamp", "page": {"limit": args.limit}});
    let found = dd.search(ctx, "/api/v2/logs/events/search", body)?;
    let rows: Vec<LogRow> = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| log_row(item, args.full))
        .collect();
    more_match(ctx, &found, rows.len(), args.limit, "dd log-count list");
    Ok(rows)
}

fn log_row(item: &Value, full: bool) -> LogRow {
    let attributes = &item["attributes"];
    let inner = &attributes["attributes"];
    let long =
        |value: &Value| text(value).map(|text| if full { text } else { cut(&text, MESSAGE_MAX) });
    let error = &inner["error"];
    let error = (error.is_object()).then(|| ErrorInfo {
        kind: text(&error["kind"]),
        message: long(&error["message"]),
    });
    LogRow {
        id: text(&item["id"]).unwrap_or_default(),
        time: utc_ms(attributes["timestamp"].as_str().unwrap_or_default()),
        status: text(&attributes["status"]),
        service: text(&attributes["service"]),
        host: text(&attributes["host"]),
        message: long(&attributes["message"]).unwrap_or_default(),
        error,
        trace_id: id_text(&inner["dd"]["trace_id"]).or_else(|| id_text(&inner["trace_id"])),
        pod: pod_ref(&attributes["tags"]),
        attributes: full.then(|| inner.clone()),
    }
}

/// A trace id Datadog wrote as a string or a number.
pub(crate) fn id_text(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        other => text(other),
    }
}

/// `--limit` fits one page; anything more is a count's job.
pub(crate) fn check_page(limit: usize, what: &str) -> Result<()> {
    if limit > PAGE_MAX {
        return Err(
            Failure::usage(format!("--limit is at most {PAGE_MAX}, one page of {what}"))
                .hint(if what == "logs" {
                    "count every match with agent-cli dd log-count list --by service"
                } else {
                    "count every span with agent-cli dd service get SERVICE"
                })
                .into(),
        );
    }
    Ok(())
}

pub(crate) fn slow_window(ctx: &Ctx, window: Window) {
    if window.seconds() > SLOW_WINDOW {
        ctx.note(
            "[a window over 1d is slow to search; counting is fast: agent-cli dd log-count list]",
        );
    }
}

/// The logs and spans APIs return a cursor, not a total.
pub(crate) fn more_match(ctx: &Ctx, found: &Value, shown: usize, limit: usize, count: &str) {
    if shown >= limit && !found["meta"]["page"]["after"].is_null() {
        ctx.note(format!(
            "[{shown} shown, more match; narrow QUERY, or count them with agent-cli {count}]"
        ));
    }
}

command! {
    pub LOG_LIST = ["dd", "log", "list"], Read,
    "Search Datadog logs in a time window, newest first, as compact rows",
    keywords: ["error", "exception", "stack", "message", "logged", "datadog logs", "last night", "yesterday", "died"],
    example: "dd log list --service api --status error --since 1h --fields time,message,pod",
    run: log_list,
}

// ---------- dd log-count list ----------

#[derive(clap::Args)]
pub struct LogCountArgs {
    /// Datadog log search syntax, ANDed with the flags
    query: Option<String>,
    /// Facets to group by: service, status, kube_namespace, pod_name, @http.status_code … (repeatable)
    #[arg(long, value_delimiter = ',', default_value = "service")]
    by: Vec<String>,
    /// From when (default 1h before --until)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
    /// Log status (repeatable)
    #[arg(long, value_delimiter = ',', ignore_case = true)]
    status: Vec<LogStatus>,
    #[command(flatten)]
    scope: Scope,
    /// Groups to return, largest first
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CountRow {
    /// The facet values this count is for.
    group: BTreeMap<String, Value>,
    count: u64,
}

fn log_count(ctx: &Ctx, args: LogCountArgs) -> Result<Vec<CountRow>> {
    let dd = Dd::load(ctx)?;
    let window = Window::new(ctx, args.since, args.until, "1h")?;
    let mut terms = args.scope.tags(&dd, true)?;
    terms.extend(any_of("status", &names(&args.status)));
    let group_by: Vec<Value> = args
        .by
        .iter()
        .map(|facet| {
            json!({
                "facet": facet,
                "limit": args.limit,
                "sort": {"aggregation": "count", "order": "desc", "type": "measure"},
            })
        })
        .collect();
    let body = json!({
        "compute": [{"aggregation": "count", "type": "total"}],
        "filter": {
            "query": search_query(args.query.as_deref(), &terms),
            "from": window.since_ms(),
            "to": window.until_ms(),
        },
        "group_by": group_by,
    });
    let found = dd.search(ctx, "/api/v2/logs/analytics/aggregate", body)?;
    let mut rows: Vec<CountRow> = found["data"]["buckets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|bucket| CountRow {
            group: bucket["by"]
                .as_object()
                .map(|by| by.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
            count: count(&bucket["computes"]["c0"]),
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.count));
    Ok(limited(ctx, rows, args.limit))
}

/// A count Datadog wrote as an integer or a float.
pub(crate) fn count(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_f64().map(|count| count.max(0.0).round() as u64))
        .unwrap_or(0)
}

command! {
    pub LOG_COUNT_LIST = ["dd", "log-count", "list"], Read,
    "Count Datadog logs grouped by facets such as service or status",
    keywords: ["count", "top", "breakdown", "most", "how many", "per", "noisiest", "group", "errors"],
    example: "dd log-count list --status error --by service,kube_namespace --fields group,count",
    run: log_count,
}

// ---------- dd event list ----------

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

    use crate::testkit::{dd, sent_bodies};

    fn log(
        id: &str,
        time: &str,
        status: &str,
        service: &str,
        pod: &str,
        message: &str,
    ) -> serde_json::Value {
        json!({
            "id": id,
            "type": "log",
            "attributes": {
                "timestamp": time,
                "status": status,
                "service": service,
                "host": "aks-nodepool1-12345678-vmss000001",
                "message": message,
                "tags": ["env:prod", "kube_cluster_name:prod", "kube_namespace:web", format!("pod_name:{pod}"), format!("service:{service}")],
                "attributes": {
                    "error": {"kind": "PSQLException", "message": "password authentication failed"},
                    "dd": {"trace_id": "5417337734251113490"}
                }
            }
        })
    }

    #[test]
    fn log_list_searches_the_window_and_prints_compact_rows_with_the_pod_id() {
        let long = "x".repeat(400);
        let (outcome, transport) = dd(
            &[
                "dd",
                "log",
                "list",
                "--service",
                "worker",
                "--status",
                "error",
                "--namespace",
                "web",
                "--since",
                "2026-09-29T11:45:00Z",
                "--until",
                "2026-09-29T12:00:00Z",
            ],
            vec![Answer::json(&json!({
                "data": [
                    log("AAAAAWgN8Xwgr1vKDQAAAABBV2dOOFh3ZzZobm1mWXJFYTR0OA", "2026-09-29T11:47:13.905Z", "error", "worker", "worker-5c4d3e9f1-q8zt1", "connect failed: FATAL: password authentication failed for user \"worker\""),
                    log("AAAAAWgN8Xwgr1vKDQAAAABBV2dOOFh3ZzZobm1mWXJFYTR0OB", "2026-09-29T11:46:00.100+00:00", "error", "worker", "worker-5c4d3e9f1-q8zt1", &long),
                ],
                "links": {"next": "https://api.datadoghq.eu/api/v2/logs/events?page[cursor]=abc"},
                "meta": {"page": {"after": "eyJhZnRlciI6IkFRQUFBWGdOOFh3Z3IxdktEUUFBQUFCQlYyZE9PRmgzWnpab2JtMW1XWEpGWVRSME9BIn0"}, "status": "done"}
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0]["id"],
            "AAAAAWgN8Xwgr1vKDQAAAABBV2dOOFh3ZzZobm1mWXJFYTR0OA"
        );
        assert_eq!(rows[0]["time"], "2026-09-29T11:47:13.905Z");
        assert_eq!(rows[0]["pod"], "prod/web/worker-5c4d3e9f1-q8zt1");
        assert_eq!(rows[0]["trace_id"], "5417337734251113490");
        assert_eq!(rows[0]["error"]["kind"], "PSQLException");
        assert!(rows[0].get("attributes").is_none(), "only with --full");
        assert!(
            rows[1]["message"]
                .as_str()
                .unwrap()
                .ends_with("\u{2026}(+100)")
        );
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            "https://api.datadoghq.eu/api/v2/logs/events/search"
        );
        assert_eq!(
            sent[0].method,
            agent_cli_core::Method::Query,
            "a search is a read"
        );
        assert_eq!(
            sent_bodies(&transport)[0],
            json!({
                "filter": {
                    "query": "service:worker env:prod kube_namespace:web status:error",
                    "from": "1790682300000",
                    "to": "1790683200000"
                },
                "sort": "-timestamp",
                "page": {"limit": 50}
            })
        );
    }

    #[test]
    fn log_list_says_when_more_match_and_full_keeps_every_attribute() {
        let page = json!({
            "data": [log("a1", "2026-09-29T11:47:13.905Z", "error", "worker", "worker-5c4d3e9f1-q8zt1", "boom")],
            "meta": {"page": {"after": "cursor-1"}}
        });
        let (outcome, _) = dd(
            &["dd", "log", "list", "--limit", "1", "--full"],
            vec![Answer::json(&page)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[0]["attributes"]["error"]["kind"],
            "PSQLException"
        );
        assert!(outcome.stderr.contains("[1 shown, more match; narrow QUERY, or count them with agent-cli dd log-count list]"), "{}", outcome.stderr);
        assert!(
            outcome
                .stderr
                .contains("[since 15m before --until by default: "),
            "{}",
            outcome.stderr
        );
        let (outcome, _) = dd(
            &["dd", "log", "list", "--since", "2d"],
            vec![Answer::json(&json!({"data": []}))],
        );
        assert_eq!(outcome.json(), json!([]));
        assert!(
            outcome.stderr.contains("window over 1d"),
            "{}",
            outcome.stderr
        );

        let (outcome, transport) = dd(&["dd", "log", "list", "--limit", "5000"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("dd log-count list"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn log_count_list_groups_by_facets_largest_first() {
        let (outcome, transport) = dd(
            &[
                "dd",
                "log-count",
                "list",
                "--status",
                "error",
                "--by",
                "service,kube_namespace",
                "--since",
                "2026-09-28T21:00:00Z",
                "--until",
                "2026-09-28T23:00:00Z",
            ],
            vec![Answer::json(&json!({
                "data": {"buckets": [
                    {"by": {"service": "api", "kube_namespace": "web"}, "computes": {"c0": 212}},
                    {"by": {"service": "worker", "kube_namespace": "web"}, "computes": {"c0": 1840}}
                ]},
                "meta": {"status": "done", "elapsed": 31}
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"group": {"kube_namespace": "web", "service": "worker"}, "count": 1840},
                {"group": {"kube_namespace": "web", "service": "api"}, "count": 212}
            ])
        );
        let body = &sent_bodies(&transport)[0];
        assert_eq!(body["filter"]["query"], "env:prod status:error");
        assert_eq!(body["filter"]["from"], "1790629200000");
        assert_eq!(body["group_by"][1]["facet"], "kube_namespace");
        assert_eq!(body["compute"][0]["aggregation"], "count");
        assert!(
            outcome.stderr.is_empty(),
            "no default window note: {}",
            outcome.stderr
        );
    }

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
