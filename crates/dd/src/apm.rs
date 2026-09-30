//! APM: which services send traces, how healthy one is (error rate and
//! latency from span aggregates Datadog computes), and the spans themselves,
//! with durations in milliseconds rather than Datadog's nanoseconds.

use agent_cli_core::{Ctx, Failure, Span, When, command, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{
    Dd, Scope, Window, any_of, cut, limited, pod_ref, search_query, strings, text, utc_ms,
};
use crate::logs::{check_page, count, id_text, more_match, names, slow_window};

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum SpanStatus {
    Ok,
    Error,
}

/// The spans a service's own endpoints start, rather than every internal
/// call: what Datadog's service page counts.
const ENTRY_SPANS: &str = "@_top_level:1";
/// The key the span aggregate puts its total bucket under.
const TOTAL: &str = "__total__";

// ---------- dd service list ----------

#[derive(clap::Args)]
pub struct ServiceListArgs {
    /// env tag (default [datadog] env, else every env)
    #[arg(long)]
    env: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn service_list(ctx: &Ctx, args: ServiceListArgs) -> Result<Vec<String>> {
    let dd = Dd::load(ctx)?;
    let env = args
        .env
        .or_else(|| dd.env.clone())
        .unwrap_or_else(|| "*".to_owned());
    let found = dd.get(ctx, "/api/v2/apm/services", &[("filter[env]", env)])?;
    let mut names = strings(&found["data"]["attributes"]["services"]);
    names.sort();
    Ok(limited(ctx, names, args.limit))
}

command! {
    pub SERVICE_LIST = ["dd", "service", "list"], Read,
    "List the APM services that send traces to Datadog",
    keywords: ["apm services", "instrumented", "traced", "which services"],
    example: "dd service list --env prod",
    run: service_list,
}

// ---------- dd service get ----------

#[derive(clap::Args)]
pub struct ServiceGetArgs {
    /// The service, as dd service list prints it
    service: String,
    /// env tag (default [datadog] env, else every env)
    #[arg(long)]
    env: Option<String>,
    /// From when (default 1h before --until)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
}

/// Counted over indexed spans, which retention filters may sample; the
/// error rate is a ratio, so sampling moves it little.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceHealth {
    id: String,
    env: Option<String>,
    since: String,
    until: String,
    /// Requests: the service's entry spans.
    spans: u64,
    errors: u64,
    /// errors / spans.
    error_rate: Option<f64>,
    p50_ms: Option<f64>,
    p95_ms: Option<f64>,
    p99_ms: Option<f64>,
    /// The busiest endpoints.
    resources: Vec<ResourceHealth>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ResourceHealth {
    resource: String,
    spans: u64,
    errors: u64,
    p95_ms: Option<f64>,
}

fn service_get(ctx: &Ctx, args: ServiceGetArgs) -> Result<ServiceHealth> {
    let service = args
        .service
        .trim()
        .trim_start_matches("service:")
        .to_owned();
    if service.is_empty() || service.contains(char::is_whitespace) {
        return Err(
            Failure::usage(format!("{:?} is not a service name", args.service))
                .hint("list them: agent-cli dd service list")
                .into(),
        );
    }
    let dd = Dd::load(ctx)?;
    let window = Window::new(ctx, args.since, args.until, "1h")?;
    let env = args.env.or_else(|| dd.env.clone());
    let mut terms = vec![format!("service:{service}")];
    terms.extend(env.as_ref().map(|env| format!("env:{env}")));
    terms.push(ENTRY_SPANS.to_owned());
    let aggregate = |extra: Option<&str>, compute: Value| -> Result<Vec<(String, Value)>> {
        let mut terms = terms.clone();
        terms.extend(extra.map(str::to_owned));
        let body = json!({"data": {"type": "aggregate_request", "attributes": {
            "compute": compute,
            "filter": {
                "query": terms.join(" "),
                "from": window.since_ms(),
                "to": window.until_ms(),
            },
            "group_by": [{
                "facet": "resource_name",
                "limit": 10,
                "sort": {"aggregation": "count", "order": "desc"},
                "total": TOTAL,
            }],
        }}});
        let found = dd.search(ctx, "/api/v2/spans/analytics/aggregate", body)?;
        Ok(found["data"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|bucket| {
                let attributes = &bucket["attributes"];
                let resource = text(&attributes["by"]["resource_name"]).unwrap_or_default();
                // Datadog's answer says `compute`; its spec, `computes`.
                let computes = if attributes["compute"].is_object() {
                    attributes["compute"].clone()
                } else {
                    attributes["computes"].clone()
                };
                (resource, computes)
            })
            .collect())
    };
    let all = aggregate(
        None,
        json!([
            {"aggregation": "count", "type": "total"},
            {"aggregation": "pc50", "metric": "@duration", "type": "total"},
            {"aggregation": "pc95", "metric": "@duration", "type": "total"},
            {"aggregation": "pc99", "metric": "@duration", "type": "total"},
        ]),
    )?;
    let failed = aggregate(
        Some("status:error"),
        json!([{"aggregation": "count", "type": "total"}]),
    )?;
    let errors_of = |resource: &str| {
        failed
            .iter()
            .find(|(name, _)| name == resource)
            .map_or(0, |(_, computes)| count(&computes["c0"]))
    };
    let total = all.iter().find(|(name, _)| name == TOTAL);
    let spans = total.map_or(0, |(_, computes)| count(&computes["c0"]));
    let errors = errors_of(TOTAL);
    Ok(ServiceHealth {
        id: service,
        env,
        since: utc_time(window.since),
        until: utc_time(window.until),
        spans,
        errors,
        error_rate: (spans > 0)
            .then(|| (errors as f64 / spans as f64 * 10_000.0).round() / 10_000.0),
        p50_ms: total.and_then(|(_, computes)| millis(&computes["c1"])),
        p95_ms: total.and_then(|(_, computes)| millis(&computes["c2"])),
        p99_ms: total.and_then(|(_, computes)| millis(&computes["c3"])),
        resources: all
            .iter()
            .filter(|(name, _)| name != TOTAL)
            .map(|(name, computes)| ResourceHealth {
                resource: name.clone(),
                spans: count(&computes["c0"]),
                errors: errors_of(name),
                p95_ms: millis(&computes["c2"]),
            })
            .collect(),
    })
}

/// Nanoseconds, as Datadog measures a span, in milliseconds to 0.1.
fn millis(nanos: &Value) -> Option<f64> {
    nanos
        .as_f64()
        .map(|nanos| (nanos / 100_000.0).round() / 10.0)
}

command! {
    pub SERVICE_GET = ["dd", "service", "get"], Read,
    "Show an APM service's health: request count, error rate, p50/p95/p99 latency",
    keywords: ["latency", "p95", "error rate", "throughput", "health", "slow", "healthy", "requests"],
    example: "dd service get api --env prod --fields error_rate,p95_ms,resources",
    run: service_get,
}

// ---------- dd span list ----------

#[derive(clap::Args)]
pub struct SpanListArgs {
    /// Datadog trace search syntax, ANDed with the flags: 'resource_name:"GET /orders"'
    query: Option<String>,
    /// From when (default 15m before --until)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
    /// Span status (repeatable)
    #[arg(long, value_delimiter = ',', ignore_case = true)]
    status: Vec<SpanStatus>,
    /// Only spans slower than this: 500ms, 2s
    #[arg(long)]
    min_duration: Option<Span>,
    /// Every span of this trace id (from a log's or span's trace_id)
    #[arg(long)]
    trace: Option<String>,
    #[command(flatten)]
    scope: Scope,
    /// At most 1000 (one page)
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SpanRow {
    /// What --trace takes, to see the whole trace.
    trace_id: String,
    span_id: String,
    /// UTC, with milliseconds.
    time: String,
    service: Option<String>,
    resource: Option<String>,
    operation: Option<String>,
    status: String,
    duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<SpanError>,
    /// The k8s pod id (cluster/namespace/pod) that `agent-cli k8s pod get` takes.
    pod: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SpanError {
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Cut to 300 characters.
    message: Option<String>,
}

fn span_list(ctx: &Ctx, args: SpanListArgs) -> Result<Vec<SpanRow>> {
    check_page(args.limit, "spans")?;
    let dd = Dd::load(ctx)?;
    let window = Window::new(ctx, args.since, args.until, "15m")?;
    slow_window(ctx, window);
    let mut terms = args.scope.tags(&dd, args.trace.is_none())?;
    terms.extend(any_of("status", &names(&args.status)));
    if let Some(min) = args.min_duration {
        terms.push(format!("@duration:>{}", min.0.as_nanos()));
    }
    if let Some(trace) = &args.trace {
        terms.push(format!("trace_id:{}", trace.trim()));
    }
    let body = json!({"data": {"type": "search_request", "attributes": {
        "filter": {
            "query": search_query(args.query.as_deref(), &terms),
            "from": window.since_ms(),
            "to": window.until_ms(),
        },
        "sort": "-timestamp",
        "page": {"limit": args.limit},
    }}});
    let found = dd.search(ctx, "/api/v2/spans/events/search", body)?;
    let rows: Vec<SpanRow> = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(span_row)
        .collect();
    more_match(
        ctx,
        &found,
        rows.len(),
        args.limit,
        "dd service get SERVICE",
    );
    Ok(rows)
}

fn span_row(item: &Value) -> SpanRow {
    let attributes = &item["attributes"];
    let custom = &attributes["custom"];
    let error = &custom["error"];
    let failed = error.is_object();
    SpanRow {
        trace_id: id_text(&attributes["trace_id"]).unwrap_or_default(),
        span_id: id_text(&attributes["span_id"]).unwrap_or_default(),
        time: utc_ms(attributes["start_timestamp"].as_str().unwrap_or_default()),
        service: text(&attributes["service"]),
        resource: text(&attributes["resource_name"]),
        operation: text(&attributes["operation_name"]).or_else(|| text(&custom["operation_name"])),
        status: if failed { "error" } else { "ok" }.to_owned(),
        duration_ms: millis(&custom["duration"]),
        error: failed.then(|| SpanError {
            kind: text(&error["type"]),
            message: text(&error["message"]).map(|message| cut(&message, 300)),
        }),
        pod: pod_ref(&attributes["tags"]),
    }
}

command! {
    pub SPAN_LIST = ["dd", "span", "list"], Read,
    "Search APM spans: failing or slow requests, or every span of one trace",
    keywords: ["trace", "slow request", "exception", "apm", "failing requests", "tracing"],
    example: "dd span list --service api --status error --since 30m --fields time,resource,error,trace_id",
    run: span_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testkit::{dd, sent_bodies};

    fn bucket(resource: &str, computes: serde_json::Value) -> serde_json::Value {
        json!({"id": format!("b-{resource}"), "type": "bucket", "attributes": {"by": {"resource_name": resource}, "compute": computes}})
    }

    #[test]
    fn service_get_computes_error_rate_and_latency_from_two_aggregates() {
        let (outcome, transport) = dd(
            &[
                "dd",
                "service",
                "get",
                "api",
                "--since",
                "2026-09-28T21:30:00Z",
                "--until",
                "2026-09-28T22:30:00Z",
            ],
            vec![
                Answer::json(&json!({"data": [
                    bucket("GET /orders/{id}", json!({"c0": 3000, "c1": 12_000_000.0, "c2": 48_000_000.0, "c3": 190_000_000.0})),
                    bucket("POST /orders", json!({"c0": 600, "c1": 30_000_000.0, "c2": 2_510_000_000.0, "c3": 3_000_000_000.0})),
                    bucket("__total__", json!({"c0": 3600, "c1": 13_000_000.0, "c2": 95_250_000.0, "c3": 2_900_000_000.0}))
                ], "meta": {"status": "done"}})),
                Answer::json(&json!({"data": [
                    bucket("POST /orders", json!({"c0": 216})),
                    bucket("__total__", json!({"c0": 216}))
                ]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({
                "id": "api", "env": "prod",
                "since": "2026-09-28T21:30:00Z", "until": "2026-09-28T22:30:00Z",
                "spans": 3600, "errors": 216, "error_rate": 0.06,
                "p50_ms": 13.0, "p95_ms": 95.3, "p99_ms": 2900.0,
                "resources": [
                    {"resource": "GET /orders/{id}", "spans": 3000, "errors": 0, "p95_ms": 48.0},
                    {"resource": "POST /orders", "spans": 600, "errors": 216, "p95_ms": 2510.0}
                ]
            })
        );
        let bodies = sent_bodies(&transport);
        let attributes = &bodies[0]["data"]["attributes"];
        assert_eq!(
            attributes["filter"]["query"],
            "service:api env:prod @_top_level:1"
        );
        assert_eq!(attributes["group_by"][0]["total"], "__total__");
        assert_eq!(
            bodies[1]["data"]["attributes"]["filter"]["query"],
            "service:api env:prod @_top_level:1 status:error"
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v2/spans/analytics/aggregate"
        );
    }

    #[test]
    fn span_list_prints_milliseconds_errors_and_the_pod_id() {
        let (outcome, transport) = dd(
            &[
                "dd",
                "span",
                "list",
                "--service",
                "api",
                "--status",
                "error",
                "--min-duration",
                "2s",
                "--since",
                "2026-09-28T21:30:00Z",
                "--until",
                "2026-09-28T22:30:00Z",
            ],
            vec![Answer::json(&json!({
                "data": [{
                    "id": "AAAAAZIxdE1234",
                    "type": "spans",
                    "attributes": {
                        "trace_id": "5417337734251113490", "span_id": "8820561924450163777", "parent_id": "0",
                        "service": "api", "resource_name": "POST /orders", "env": "prod",
                        "host": "aks-nodepool1-12345678-vmss000001", "type": "web",
                        "start_timestamp": "2026-09-28T21:36:12.345Z", "end_timestamp": "2026-09-28T21:36:14.855Z",
                        "tags": ["env:prod", "kube_cluster_name:prod", "kube_namespace:web", "pod_name:api-7d9f8c6b5-x2k4q"],
                        "custom": {
                            "duration": 2_510_000_000_i64,
                            "operation_name": "aspnet_core.request",
                            "error": {"type": "HttpRequestException", "message": "upstream worker unavailable (503)"}
                        }
                    }
                }],
                "meta": {"page": {}}
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "trace_id": "5417337734251113490", "span_id": "8820561924450163777",
                "time": "2026-09-28T21:36:12.345Z", "service": "api", "resource": "POST /orders",
                "operation": "aspnet_core.request", "status": "error", "duration_ms": 2510.0,
                "error": {"type": "HttpRequestException", "message": "upstream worker unavailable (503)"},
                "pod": "prod/web/api-7d9f8c6b5-x2k4q"
            }])
        );
        let body = &sent_bodies(&transport)[0];
        assert_eq!(body["data"]["type"], "search_request");
        assert_eq!(
            body["data"]["attributes"]["filter"]["query"],
            "service:api env:prod status:error @duration:>2000000000"
        );

        let (outcome, transport) = dd(
            &[
                "dd",
                "span",
                "list",
                "--trace",
                "5417337734251113490",
                "--since",
                "1d",
            ],
            vec![Answer::json(&json!({"data": []}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            sent_bodies(&transport)[0]["data"]["attributes"]["filter"]["query"],
            "trace_id:5417337734251113490",
            "a trace is every span of it, in any env"
        );
    }
}
