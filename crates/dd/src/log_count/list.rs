use std::collections::BTreeMap;

use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Dd, Scope, Window, any_of, limited, search_query};
use crate::search::{LogStatus, count, names};

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

command! {
    pub LOG_COUNT_LIST = ["dd", "log-count", "list"], Read,
    "Count Datadog logs grouped by facets such as service or status",
    keywords: ["count", "top", "breakdown", "most", "how many", "per", "noisiest", "group", "errors"],
    example: "dd log-count list --status error --by service,kube_namespace --fields group,count",
    run: log_count,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{dd, sent_bodies};

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
}
