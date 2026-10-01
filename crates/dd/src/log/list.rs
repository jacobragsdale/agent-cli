use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Dd, Scope, Window, any_of, cut, pod_ref, search_query, text, utc_ms};
use crate::search::{
    LogStatus, MESSAGE_MAX, at, check_page, id_text, more_match, names, slow_window,
};

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
    /// The innermost frame of error.stack in the service: PATH:LINE, or
    /// PROJECT/REPO@SHA:PATH:LINE with the git tags; what ado file get takes.
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<String>,
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
        at: at(&inner["error"]["stack"], &attributes["tags"]),
        trace_id: id_text(&inner["dd"]["trace_id"]).or_else(|| id_text(&inner["trace_id"])),
        pod: pod_ref(&attributes["tags"]),
        attributes: full.then(|| inner.clone()),
    }
}

command! {
    pub LOG_LIST = ["dd", "log", "list"], Read,
    "Search Datadog logs in a time window, newest first, as compact rows",
    keywords: ["error", "exception", "stack", "message", "logged", "datadog logs", "last night", "yesterday", "died", "production", "line"],
    example: "dd log list --service api --status error --since 1h --fields time,message,pod",
    run: log_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{dd, sent_bodies};

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
}
