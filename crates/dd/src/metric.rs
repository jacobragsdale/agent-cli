//! Metrics: finding a metric's name, and querying it as a summary an agent
//! can read (stats and a dozen points per series) instead of raw pointlists.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Failure, When, command, utc_time};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;

use crate::client::{Dd, Scope, Window, limited, strings, text};

// ---------- dd metric list ----------

#[derive(clap::Args)]
pub struct MetricListArgs {
    /// Part of the metric name: kubernetes.memory, latency
    pattern: Option<String>,
    /// Reporting since when (default 1h ago)
    #[arg(long)]
    since: Option<When>,
    #[command(flatten)]
    scope: Scope,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn metric_list(ctx: &Ctx, args: MetricListArgs) -> Result<Vec<String>> {
    let dd = Dd::load(ctx)?;
    let since = args.since.map_or_else(
        || agent_cli_core::now() - time::Duration::hours(1),
        |since| since.0,
    );
    let mut query = vec![("from", since.unix_timestamp().to_string())];
    let tags = args.scope.tags(&dd, false)?;
    if !tags.is_empty() {
        query.push(("tag_filter", tags.join(" AND ")));
    }
    let found = dd.get(ctx, "/api/v1/metrics", &query)?;
    let mut names: Vec<String> = strings(&found["metrics"])
        .into_iter()
        .filter(|name| {
            args.pattern
                .as_deref()
                .is_none_or(|pattern| name.contains(pattern))
        })
        .collect();
    names.sort();
    Ok(limited(ctx, names, args.limit))
}

command! {
    pub METRIC_LIST = ["dd", "metric", "list"], Read,
    "Find Datadog metric names reporting recently, by part of the name",
    keywords: ["names", "available", "find metric", "which metrics", "exist"],
    example: "dd metric list kubernetes.memory --namespace web",
    run: metric_list,
}

// ---------- dd metric get ----------

#[derive(clap::Args)]
pub struct MetricGetArgs {
    /// A metric query: 'avg:kubernetes.cpu.usage.total{*} by {pod_name}'; arithmetic and .rollup() work
    query: String,
    /// From when (default 1h before --until)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
    /// Filters go inside every {…} that follows a metric name
    #[command(flatten)]
    scope: Scope,
    /// Points per series after downsampling by bucket mean (0: stats only)
    #[arg(long, default_value_t = 12)]
    points: usize,
    /// Series to return, ordered by --sort
    #[arg(long, default_value_t = 20)]
    limit: usize,
    /// The stat that orders the series, largest first
    #[arg(long, default_value = "max", value_parser = PossibleValuesParser::new(["max", "avg", "last"]))]
    sort: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SeriesRow {
    series: String,
    /// The `by {…}` tags of this series.
    group: BTreeMap<String, String>,
    /// The k8s pod id (cluster/namespace/pod), when the group names a pod.
    #[serde(skip_serializing_if = "Option::is_none")]
    pod: Option<String>,
    unit: Option<String>,
    min: Option<f64>,
    max: Option<f64>,
    /// When the series hit its max: a spike's time, which bucket means blur.
    max_at: Option<String>,
    avg: Option<f64>,
    last: Option<f64>,
    /// `[time, value]`, bucket means over the window.
    points: Vec<(String, f64)>,
    /// Points with no value.
    gaps: usize,
}

fn metric_get(ctx: &Ctx, args: MetricGetArgs) -> Result<Vec<SeriesRow>> {
    let dd = Dd::load(ctx)?;
    let window = Window::new(ctx, args.since, args.until, "1h")?;
    let query = scoped(&args.query, &args.scope.tags(&dd, false)?);
    let found = dd.get(
        ctx,
        "/api/v1/query",
        &[
            ("from", window.since.unix_timestamp().to_string()),
            ("to", window.until.unix_timestamp().to_string()),
            ("query", query.clone()),
        ],
    )?;
    if found["status"] == "error" {
        return Err(Failure::usage(format!(
            "Datadog refused the query {query:?}: {}",
            text(&found["error"]).unwrap_or_else(|| "no reason given".to_owned())
        ))
        .hint("check the metric name with agent-cli dd metric list PATTERN")
        .into());
    }
    let mut rows: Vec<SeriesRow> = found["series"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|series| series_row(series, args.points))
        .collect();
    let stat = |row: &SeriesRow| {
        match args.sort.as_str() {
            "avg" => row.avg,
            "last" => row.last,
            _ => row.max,
        }
        .unwrap_or(f64::NEG_INFINITY)
    };
    rows.sort_by(|a, b| stat(b).total_cmp(&stat(a)));
    if rows.len() > args.limit {
        ctx.note(format!(
            "[{} of {} series; add a tag inside {{}} or --limit N]",
            args.limit,
            rows.len()
        ));
        rows.truncate(args.limit);
    }
    if rows.is_empty() {
        ctx.note("[no series: nothing reported in the window, or the scope matched nothing]");
    }
    Ok(rows)
}

fn series_row(series: &Value, points: usize) -> SeriesRow {
    let pointlist: Vec<(f64, Option<f64>)> = series["pointlist"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|point| Some((point.get(0)?.as_f64()?, point.get(1)?.as_f64())))
        .collect();
    let values: Vec<f64> = pointlist.iter().filter_map(|(_, value)| *value).collect();
    let group: BTreeMap<String, String> = strings(&series["tag_set"])
        .into_iter()
        .filter_map(|tag| {
            let (key, value) = tag.split_once(':')?;
            Some((key.to_owned(), value.to_owned()))
        })
        .collect();
    let pod = crate::client::pod_ref(&series["tag_set"]);
    let peak = pointlist
        .iter()
        .filter_map(|(at, value)| Some((*at, (*value)?)))
        .reduce(|best, point| if point.1 > best.1 { point } else { best });
    SeriesRow {
        series: text(&series["display_name"])
            .or_else(|| text(&series["metric"]))
            .unwrap_or_default(),
        group,
        pod,
        unit: series["unit"]
            .as_array()
            .and_then(|units| units.first())
            .and_then(|unit| text(&unit["name"])),
        min: values.iter().copied().reduce(f64::min).map(round),
        max: peak.map(|(_, value)| round(value)),
        max_at: peak.and_then(|(at, _)| at_ms(at)),
        avg: (!values.is_empty()).then(|| round(values.iter().sum::<f64>() / values.len() as f64)),
        last: values.last().copied().map(round),
        points: downsample(&pointlist, points),
        gaps: pointlist.len() - values.len(),
    }
}

/// `points` buckets of consecutive points, each the mean of its values at
/// its first point's time; a bucket with no values is left out.
fn downsample(pointlist: &[(f64, Option<f64>)], points: usize) -> Vec<(String, f64)> {
    if points == 0 || pointlist.is_empty() {
        return Vec::new();
    }
    let size = pointlist.len().div_ceil(points);
    pointlist
        .chunks(size)
        .filter_map(|chunk| {
            let values: Vec<f64> = chunk.iter().filter_map(|(_, value)| *value).collect();
            if values.is_empty() {
                return None;
            }
            Some((
                at_ms(chunk[0].0)?,
                round(values.iter().sum::<f64>() / values.len() as f64),
            ))
        })
        .collect()
}

/// A pointlist's epoch milliseconds as a printed time.
fn at_ms(millis: f64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp((millis / 1000.0) as i64)
        .ok()
        .map(utc_time)
}

/// Four significant digits: the part of a metric an agent reasons about.
fn round(value: f64) -> f64 {
    if value == 0.0 || !value.is_finite() {
        return value;
    }
    let digits = 3 - value.abs().log10().floor() as i32;
    if digits >= 0 {
        let scale = 10f64.powi(digits);
        (value * scale).round() / scale
    } else {
        let scale = 10f64.powi(-digits);
        (value / scale).round() * scale
    }
}

/// `tags` put inside every `{…}` that follows a metric name, replacing `*`
/// and joining existing tags; a `by {…}` group is left alone.
// ponytail: a brace scanner, not a parser; a brace inside a quoted tag value
// would confuse it. Datadog tag values cannot hold braces, so none has yet.
fn scoped(query: &str, tags: &[String]) -> String {
    if tags.is_empty() {
        return query.to_owned();
    }
    let joined = tags.join(",");
    let mut out = String::with_capacity(query.len() + joined.len());
    let mut rest = query;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|at| open + at) else {
            break;
        };
        let before = &rest[..open];
        let inner = &rest[open + 1..close];
        out.push_str(before);
        out.push('{');
        let word = before.trim_end();
        let by = word.ends_with("by")
            && !word[..word.len() - 2]
                .ends_with(|c: char| c.is_ascii_alphanumeric() || "._:".contains(c));
        let metric =
            !by && before.ends_with(|c: char| c.is_ascii_alphanumeric() || "._".contains(c));
        if metric {
            match inner.trim() {
                "" | "*" => out.push_str(&joined),
                tags => {
                    out.push_str(tags);
                    out.push(',');
                    out.push_str(&joined);
                }
            }
        } else {
            out.push_str(inner);
        }
        out.push('}');
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

command! {
    pub METRIC_GET = ["dd", "metric", "get"], Read,
    "Query a Datadog metric: min, max, avg, last and a few points per series",
    keywords: ["timeseries", "graph", "cpu", "memory", "rollup", "trend", "usage", "growing", "over time", "across"],
    example: "dd metric get 'avg:kubernetes.cpu.usage.total{*} by {pod_name}' --namespace web --since 3h --fields group,max,last",
    run: metric_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::testkit::dd;

    #[test]
    fn filters_go_inside_each_metric_scope_and_never_into_by() {
        let tags = vec!["kube_namespace:web".to_owned(), "pod_name:api-1".to_owned()];
        for (query, want) in [
            ("avg:cpu{*}", "avg:cpu{kube_namespace:web,pod_name:api-1}"),
            ("avg:cpu{}", "avg:cpu{kube_namespace:web,pod_name:api-1}"),
            (
                "avg:cpu{env:prod} by {pod_name}",
                "avg:cpu{env:prod,kube_namespace:web,pod_name:api-1} by {pod_name}",
            ),
            (
                "avg:cpu{*}by{host}",
                "avg:cpu{kube_namespace:web,pod_name:api-1}by{host}",
            ),
            (
                "avg:system.standby{*}",
                "avg:system.standby{kube_namespace:web,pod_name:api-1}",
            ),
            (
                "sum:a.errors{*}.as_count() / sum:a.hits{*}.as_count()",
                "sum:a.errors{kube_namespace:web,pod_name:api-1}.as_count() / sum:a.hits{kube_namespace:web,pod_name:api-1}.as_count()",
            ),
            (
                "avg:cpu{*}.rollup(max, 60)",
                "avg:cpu{kube_namespace:web,pod_name:api-1}.rollup(max, 60)",
            ),
            (
                "avg:cpu{*}, avg:mem{*} by {pod_name}",
                "avg:cpu{kube_namespace:web,pod_name:api-1}, avg:mem{kube_namespace:web,pod_name:api-1} by {pod_name}",
            ),
        ] {
            assert_eq!(scoped(query, &tags), want, "{query}");
        }
        assert_eq!(scoped("avg:cpu{*}", &[]), "avg:cpu{*}");
    }

    #[test]
    fn a_series_is_summarised_with_gaps_counted_and_points_downsampled() {
        let pointlist: Vec<(f64, Option<f64>)> = (0..24)
            .map(|i| {
                (
                    1_790_629_200_000.0 + f64::from(i) * 300_000.0,
                    (i != 5).then_some(f64::from(i)),
                )
            })
            .collect();
        let points = downsample(&pointlist, 12);
        assert_eq!(points.len(), 12);
        assert_eq!(points[0], ("2026-09-28T21:00:00Z".to_owned(), 0.5));
        assert_eq!(points[2].1, 4.0, "the null is skipped, not averaged as 0");
        assert!(downsample(&pointlist, 0).is_empty());
        assert_eq!(downsample(&pointlist, 100).len(), 23);
        assert_eq!(round(0.123_456), 0.1235);
        assert_eq!(round(98_765.4), 98_770.0);
    }

    #[test]
    fn metric_get_returns_stats_sorted_and_capped_with_a_note() {
        let series = |pod: &str, values: &[Option<f64>]| {
            json!({
                "metric": "trace.http.request.errors",
                "display_name": "trace.http.request.errors",
                "scope": format!("pod_name:{pod},service:api"),
                "tag_set": [format!("pod_name:{pod}"), "kube_namespace:web", "kube_cluster_name:prod"],
                "expression": format!("sum:trace.http.request.errors{{pod_name:{pod},service:api}}"),
                "pointlist": values.iter().enumerate().map(|(i, v)| json!([1_790_631_000_000.0 + i as f64 * 60_000.0, v])).collect::<Vec<_>>(),
                "unit": [{"family": "network", "name": "request", "plural": "requests", "scale_factor": 1.0}, null],
                "interval": 60,
                "length": values.len()
            })
        };
        let (outcome, transport) = dd(
            &[
                "dd",
                "metric",
                "get",
                "sum:trace.http.request.errors{service:api} by {pod_name}.as_count()",
                "--namespace",
                "web",
                "--since",
                "2026-09-28T21:30:00Z",
                "--until",
                "2026-09-28T22:30:00Z",
                "--limit",
                "1",
                "--points",
                "2",
            ],
            vec![Answer::json(&json!({
                "status": "ok",
                "res_type": "time_series",
                "series": [
                    series("api-7d9f8c6b5-m3n5p", &[Some(1.0), Some(2.0)]),
                    series("api-7d9f8c6b5-x2k4q", &[Some(3.0), None, Some(40.0), Some(1.0)])
                ],
                "from_date": 1_790_631_000_000_i64,
                "to_date": 1_790_634_600_000_i64
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "series": "trace.http.request.errors",
                "group": {"kube_cluster_name": "prod", "kube_namespace": "web", "pod_name": "api-7d9f8c6b5-x2k4q"},
                "pod": "prod/web/api-7d9f8c6b5-x2k4q",
                "unit": "request",
                "min": 1.0, "max": 40.0, "max_at": "2026-09-28T21:32:00Z", "avg": 14.67, "last": 1.0,
                "points": [["2026-09-28T21:30:00Z", 3.0], ["2026-09-28T21:32:00Z", 20.5]],
                "gaps": 1
            }])
        );
        assert!(
            outcome
                .stderr
                .contains("[1 of 2 series; add a tag inside {} or --limit N]"),
            "{}",
            outcome.stderr
        );
        let url = &transport.sent()[0].url;
        assert!(
            url.starts_with(
                "https://api.datadoghq.eu/api/v1/query?from=1790631000&to=1790634600&query="
            ),
            "{url}"
        );
        assert!(
            url.contains("service%3Aapi%2Ckube_namespace%3Aweb%7D+by+%7Bpod_name%7D"),
            "{url}"
        );
    }

    #[test]
    fn a_query_datadog_refuses_is_a_usage_error_and_metric_list_filters_names() {
        let (outcome, _) = dd(
            &["dd", "metric", "get", "avg:nope{*"],
            vec![Answer::json(
                &json!({"status": "error", "error": "Error parsing query: unexpected end"}),
            )],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Error parsing query"),
            "{}",
            outcome.stderr
        );

        let (outcome, transport) = dd(
            &[
                "dd",
                "metric",
                "list",
                "memory",
                "--cluster",
                "prod",
                "--since",
                "2026-09-29T11:00:00Z",
            ],
            vec![Answer::json(
                &json!({"from": "1790679600", "metrics": ["kubernetes.memory.usage", "kubernetes.cpu.usage.total", "kubernetes.memory.limits"]}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!(["kubernetes.memory.limits", "kubernetes.memory.usage"])
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v1/metrics?from=1790679600&tag_filter=kube_cluster_name%3Aprod"
        );
    }
}
