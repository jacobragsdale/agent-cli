use agent_cli_core::{Ctx, Span, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Dd, Scope, Window, any_of, cut, pod_ref, search_query, text, utc_ms};
use crate::search::{check_page, id_text, millis, more_match, names, slow_window};

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum SpanStatus {
    Ok,
    Error,
}

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

    use crate::testing::{dd, sent_bodies};

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
