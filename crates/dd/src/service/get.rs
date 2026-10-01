use agent_cli_core::{Ctx, Failure, When, command, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Dd, Window, text};
use crate::search::{count, millis};

/// The spans a service's own endpoints start, rather than every internal
/// call: what Datadog's service page counts.
const ENTRY_SPANS: &str = "@_top_level:1";
/// The key the span aggregate puts its total bucket under.
const TOTAL: &str = "__total__";

#[derive(clap::Args)]
pub struct ServiceGetArgs {
    /// The service, as dd service list prints it
    service: String,
    /// env tag (default [datadog] env, else every env)
    #[arg(long)]
    env: Option<String>,
    /// From when: a deploy's or rollout's time, to see what it changed (default 1h before --until)
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
    if args.since.is_none() {
        ctx.note(format!(
            "[the last hour only; since a deploy: agent-cli dd service get {service} --since <updated>, the deploy's time as k8s deployment list prints it]"
        ));
    }
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

command! {
    pub SERVICE_GET = ["dd", "service", "get"], Read,
    "Show an APM service's health --since a deploy, else the last hour: errors, p95",
    keywords: ["latency", "p50", "p95", "p99", "error rate", "throughput", "health", "slow", "healthy", "requests"],
    example: "dd service get api --since 2026-09-28T21:34:40Z --fields errors,error_rate,p95_ms,resources",
    run: service_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{dd, sent_bodies};

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
}
