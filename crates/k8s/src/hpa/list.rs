use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, items, limited, name_has, non_empty};
use crate::shape::{Condition, conditions};

#[derive(clap::Args)]
pub struct HpaListArgs {
    /// Part of the autoscaler's name (usually its workload's)
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HpaRow {
    /// `cluster/namespace/name`: what `k8s object get hpa` takes.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// What it scales: Deployment/reports-api.
    target: String,
    min: i64,
    max: i64,
    current: i64,
    desired: i64,
    metrics: Vec<Metric>,
    /// AbleToScale, ScalingActive and ScalingLimited, with their reasons:
    /// TooManyReplicas when it wants more than `max`.
    conditions: Vec<Condition>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Metric {
    /// cpu, memory, or a custom or external metric's name.
    metric: String,
    /// `92%` of the pods' requests, or a value: `250m`, `1k`.
    current: Option<String>,
    target: Option<String>,
}

fn hpa_list(ctx: &Ctx, args: HpaListArgs) -> Result<Vec<HpaRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "horizontalpodautoscalers", "-o", "json"])?;
    let rows = items(&listed)
        .filter(|item| name_has(item, args.name.as_deref()))
        .filter_map(|item| {
            let spec = &item["spec"];
            let status = &item["status"];
            let scaled = &spec["scaleTargetRef"];
            Some(HpaRow {
                id: target.id(item),
                name: item["metadata"]["name"].as_str()?.to_owned(),
                namespace: target.row_namespace(item),
                target: format!(
                    "{}/{}",
                    scaled["kind"].as_str().unwrap_or_default(),
                    scaled["name"].as_str().unwrap_or_default()
                ),
                min: spec["minReplicas"].as_i64().unwrap_or(1),
                max: spec["maxReplicas"].as_i64().unwrap_or_default(),
                current: status["currentReplicas"].as_i64().unwrap_or_default(),
                desired: status["desiredReplicas"].as_i64().unwrap_or_default(),
                metrics: metrics(item),
                conditions: conditions(item),
            })
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

/// Each metric it scales on, its target from the spec and its reading from
/// the status, matched by source and name (autoscaling/v2).
fn metrics(item: &Value) -> Vec<Metric> {
    let current: Vec<(String, &Value)> = item["status"]["currentMetrics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|held| {
            let (key, name) = source(held)?;
            Some((format!("{key}/{name}"), &held[key]["current"]))
        })
        .collect();
    item["spec"]["metrics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|held| {
            let (key, name) = source(held)?;
            let wanted = format!("{key}/{name}");
            Some(Metric {
                metric: name.to_owned(),
                current: current
                    .iter()
                    .find(|(held, _)| *held == wanted)
                    .and_then(|(_, reading)| amount(reading)),
                target: amount(&held[key]["target"]),
            })
        })
        .collect()
}

/// A metric's source key (resource, pods, object, external,
/// containerResource) and its name.
fn source(metric: &Value) -> Option<(&'static str, &str)> {
    let key = match metric["type"].as_str()? {
        "Resource" => "resource",
        "Pods" => "pods",
        "Object" => "object",
        "External" => "external",
        "ContainerResource" => "containerResource",
        _ => return None,
    };
    let held = &metric[key];
    let name = non_empty(&held["name"]).or_else(|| non_empty(&held["metric"]["name"]))?;
    Some((key, name))
}

/// A target or reading: a utilization as a percentage of the pods'
/// requests, else an average or a total value as Kubernetes writes it.
fn amount(held: &Value) -> Option<String> {
    if let Some(percent) = held["averageUtilization"].as_i64() {
        return Some(format!("{percent}%"));
    }
    ["averageValue", "value"]
        .iter()
        .find_map(|key| non_empty(&held[*key]).map(str::to_owned))
}

command! {
    pub HPA_LIST = ["k8s", "hpa", "list"], Read,
    "List autoscalers: min, max, current and desired replicas, metrics vs target",
    keywords: ["horizontalpodautoscaler", "autoscaler", "autoscale", "autoscaling", "maximum", "more", "pods", "cpu", "utilization", "scalinglimited", "toomanyreplicas"],
    example: "k8s hpa list --cluster prod --namespace web --fields id,current,max,metrics,conditions",
    run: hpa_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn hpa_list_shows_each_metric_against_its_target_and_why_scaling_is_limited() {
        let outcome = run(&["k8s", "hpa", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "qa/dev/orders-api", "name": "orders-api", "target": "Deployment/orders-api",
                "min": 2, "max": 4, "current": 4, "desired": 4,
                "metrics": [{"metric": "cpu", "current": "88%", "target": "70%"},
                    {"metric": "memory", "current": "300Mi", "target": "512Mi"}],
                "conditions": [
                    {"type": "AbleToScale", "status": "True", "reason": "ReadyForNewScale"},
                    {"type": "ScalingLimited", "status": "True", "reason": "TooManyReplicas",
                        "message": "the desired replica count is more than the maximum replica count"}]}])
        );
    }
}
