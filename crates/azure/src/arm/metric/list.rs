//! `arm metric list`: Azure Monitor's platform metrics on any resource,
//! `metrics` for the ones named, else `metricDefinitions`, so an agent can
//! pick a name before it asks.

use agent_cli_core::{Ctx, Failure, Request, Span, When, command, percent_encode};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::arm::resource::any;
use crate::client::{ARM, arm_only, bearer, limited, refused_with, stamp, text};
use crate::config::Azure;
use crate::net::{list, strings};

const API: &str = "2018-01-01";
const AGGREGATIONS: [&str; 5] = ["average", "minimum", "maximum", "total", "count"];

#[derive(clap::Args)]
pub struct MetricListArgs {
    /// The resource: its ARM id (k8s pvc get's disk, k8s node get's vm), portal link or name
    resource: String,
    /// A metric to read, as the list without it names it: "Composite Disk Read Operations/sec" (repeatable)
    #[arg(long)]
    metric: Vec<String>,
    /// Its resource group, for a name two groups hold
    #[arg(long)]
    group: Option<String>,
    /// From when (default 1h before --until)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
    /// One point per this much: 1m, 5m, 15m, 1h, 1d (default Azure's for the window)
    #[arg(long)]
    interval: Option<Span>,
    /// How a point combines the samples in it (default the metric's own)
    #[arg(long, value_parser = PossibleValuesParser::new(AGGREGATIONS))]
    aggregation: Option<String>,
    /// The newest points per metric, or the definitions, to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MetricRow {
    /// What --metric takes.
    metric: String,
    unit: Option<String>,
    /// Without --metric, what it can be read as, its default first.
    aggregations: Vec<String>,
    /// Oldest first, the newest --limit of them.
    points: Vec<Point>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Point {
    at: Option<String>,
    value: f64,
}

/// A span in ISO 8601, as Azure Monitor takes an interval: `PT5M`, `P1D`.
fn iso(span: Span) -> String {
    let seconds = span.0.as_secs();
    if seconds > 0 && seconds.is_multiple_of(86_400) {
        return format!("P{}D", seconds / 86_400);
    }
    let mut out = String::from("PT");
    for (count, unit) in [
        (seconds / 3600, 'H'),
        (seconds % 3600 / 60, 'M'),
        (seconds % 60, 'S'),
    ] {
        if count > 0 {
            out.push_str(&format!("{count}{unit}"));
        }
    }
    if out == "PT" {
        out.push_str("0S");
    }
    out
}

fn encoded(raw: &str) -> String {
    let mut out = String::new();
    percent_encode(raw, &mut out);
    out.replace('+', "%20")
}

fn metric_list(ctx: &Ctx, args: MetricListArgs) -> Result<Vec<MetricRow>> {
    let azure = Azure::load(ctx)?;
    let target = any(ctx, &azure, &args.resource, args.group.as_deref())?;
    let base = format!(
        "https://management.azure.com{}/providers/Microsoft.Insights",
        target.id
    );
    let get = |url: String| -> Result<Value> {
        ctx.read(Request::get(arm_only(&url)?).auth(&bearer(ctx, ARM)))
            .map_err(|error| {
                if refused_with(&error) == Some(400) && !args.metric.is_empty() {
                    return Failure::usage(format!("{error:#}"))
                        .hint(format!(
                            "the metric names: agent-cli arm metric list {}",
                            target.id
                        ))
                        .into();
                }
                error
            })?
            .json()
    };
    if args.metric.is_empty() {
        let answer = get(format!("{base}/metricDefinitions?api-version={API}"))?;
        let rows = list(&answer["value"])
            .iter()
            .map(|held| {
                let primary = text(&held["primaryAggregationType"]);
                let mut aggregations: Vec<String> = primary.iter().cloned().collect();
                for other in strings(&held["supportedAggregationTypes"]) {
                    // `None` is a raw sample, which --aggregation cannot ask for.
                    if other != "None" && !aggregations.contains(&other) {
                        aggregations.push(other);
                    }
                }
                MetricRow {
                    metric: text(&held["name"]["value"]).unwrap_or_default(),
                    unit: text(&held["unit"]),
                    aggregations,
                    points: Vec::new(),
                }
            })
            .collect();
        return Ok(limited(ctx, rows, args.limit));
    }
    let until = args.until.unwrap_or_else(When::now);
    let since = args
        .since
        .unwrap_or(When(until.0 - time::Duration::hours(1)));
    let mut url = format!(
        "{base}/metrics?api-version={API}&metricnames={}&timespan={}",
        encoded(&args.metric.join(",")),
        encoded(&format!("{}/{}", since.utc(), until.utc()))
    );
    if let Some(interval) = args.interval {
        url.push_str(&format!("&interval={}", iso(interval)));
    }
    if let Some(aggregation) = &args.aggregation {
        url.push_str(&format!("&aggregation={aggregation}"));
    }
    let answer = get(url)?;
    let wanted: Vec<&str> = args
        .aggregation
        .as_deref()
        .map_or(AGGREGATIONS.to_vec(), |one| vec![one]);
    Ok(list(&answer["value"])
        .iter()
        .map(|held| {
            let metric = text(&held["name"]["value"]).unwrap_or_default();
            // Without a dimension filter Azure answers one series.
            let mut points: Vec<Point> = list(&held["timeseries"][0]["data"])
                .iter()
                .filter_map(|point| {
                    Some(Point {
                        at: stamp(&point["timeStamp"]),
                        value: wanted.iter().find_map(|key| point[*key].as_f64())?,
                    })
                })
                .collect();
            if points.len() > args.limit {
                ctx.note(format!(
                    "[{metric}: the newest {} of {} points; --interval or --limit N]",
                    args.limit,
                    points.len()
                ));
                points.drain(..points.len() - args.limit);
            }
            MetricRow {
                metric,
                unit: text(&held["unit"]),
                aggregations: args.aggregation.iter().cloned().collect(),
                points,
            }
        })
        .collect())
}

command! {
    pub METRIC_LIST = ["arm", "metric", "list"], Read,
    "Get Azure Monitor metric points for any Azure resource, or list its metrics",
    keywords: ["monitor", "iops", "throttled", "throttling", "cpu", "percentage", "consumed", "bandwidth", "latency", "insights", "disk"],
    example: "arm metric list pvc-7c3e9a1b-4d2f-4e8a-9b6c-1f0e2d3c4b5a --metric 'Composite Disk Write Operations/sec' --since 3h --interval 5m --fields metric,points",
    run: metric_list,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::testing::azure;

    const DISK: &str = "/subscriptions/00000000-0000-4000-8000-000000000001/resourceGroups/rg-contoso/providers/Microsoft.Compute/disks/disk-1";

    #[test]
    fn an_interval_is_sent_in_iso_8601() {
        let span = |seconds| Span(Duration::from_secs(seconds));
        assert_eq!(iso(span(300)), "PT5M");
        assert_eq!(iso(span(5400)), "PT1H30M");
        assert_eq!(iso(span(86_400)), "P1D");
        assert_eq!(iso(span(30)), "PT30S");
    }

    #[test]
    fn without_a_metric_the_definitions_come_with_their_default_aggregation_first() {
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "metric", "list", DISK],
            vec![Answer::json(
                &json!({"value": [{"name": {"value": "Composite Disk Read Operations/sec"},
                "unit": "CountPerSecond", "primaryAggregationType": "Average",
                "supportedAggregationTypes": ["Minimum", "Average", "Maximum"]}]}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"metric": "Composite Disk Read Operations/sec", "unit": "CountPerSecond",
                "aggregations": ["Average", "Minimum", "Maximum"]}])
        );
        assert_eq!(
            transport.sent()[0].url,
            format!(
                "https://management.azure.com{DISK}/providers/Microsoft.Insights/metricDefinitions?api-version=2018-01-01"
            )
        );
    }

    #[test]
    fn points_keep_the_newest_with_a_note_and_a_wrong_name_names_the_list() {
        let data: Vec<_> = (0..4)
            .map(|minute| json!({"timeStamp": format!("2026-09-29T11:0{minute}:00Z"), "maximum": f64::from(minute) * 10.0}))
            .chain([json!({"timeStamp": "2026-09-29T11:04:00Z"})])
            .collect();
        let (outcome, transport) = azure(
            &[crate::ARM],
            &[
                "arm",
                "metric",
                "list",
                DISK,
                "--metric",
                "Composite Disk Read Operations/sec",
                "--since",
                "2026-09-29T11:00:00Z",
                "--until",
                "2026-09-29T12:00:00Z",
                "--interval",
                "1m",
                "--aggregation",
                "maximum",
                "--limit",
                "2",
            ],
            vec![Answer::json(
                &json!({"value": [{"name": {"value": "Composite Disk Read Operations/sec"},
                "unit": "CountPerSecond", "timeseries": [{"data": data}]}]}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[0]["points"],
            json!([{"at": "2026-09-29T11:02:00Z", "value": 20.0}, {"at": "2026-09-29T11:03:00Z", "value": 30.0}])
        );
        assert!(
            outcome.stderr.contains("the newest 2 of 4 points"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent()[0].url,
            format!(
                "https://management.azure.com{DISK}/providers/Microsoft.Insights/metrics?api-version=2018-01-01&metricnames=Composite%20Disk%20Read%20Operations%2Fsec&timespan=2026-09-29T11%3A00%3A00Z%2F2026-09-29T12%3A00%3A00Z&interval=PT1M&aggregation=maximum"
            )
        );

        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "metric", "list", DISK, "--metric", "IOPS"],
            vec![Answer::status(
                400,
                r#"{"code":"BadRequest","message":"Failed to find metric configuration for provider: Microsoft.Compute, resource Type: disks, metric: IOPS"}"#,
            )],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("arm metric list /subscriptions/"),
            "{}",
            outcome.stderr
        );
    }
}
