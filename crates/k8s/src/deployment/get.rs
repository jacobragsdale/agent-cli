use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, items, non_empty};
use crate::pod::{Probe, Volume, first_unready, probes, sized, volumes};
use crate::shape::{Condition, EventRow, conditions, events_in};

use super::{DeploymentRow, Revision, own_pods, revisions, row};

#[derive(clap::Args)]
pub struct DeploymentGetArgs {
    /// The deployment: its id (cluster/namespace/name), namespace/name, or name
    deployment: String,
    #[command(flatten)]
    at: At,
}

/// A deployment described: its row, how it rolls out, its history and what
/// its pod template asks for.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DeploymentDetail {
    #[serde(flatten)]
    row: DeploymentRow,
    /// `RollingUpdate (max surge 25%, max unavailable 25%)` or `Recreate`.
    strategy: Option<String>,
    /// Available and Progressing: ProgressDeadlineExceeded,
    /// MinimumReplicasUnavailable…
    conditions: Vec<Condition>,
    /// Its ReplicaSets, newest revision first: what a rollback goes back to.
    revisions: Vec<Revision>,
    /// The pod template's containers: images and the environment's names.
    containers: Vec<TemplateContainer>,
    probes: Vec<Probe>,
    /// Container to its resource requests: `{"api": {"cpu": "250m"}}`.
    requests: BTreeMap<String, BTreeMap<String, String>>,
    /// Container to its resource limits.
    limits: BTreeMap<String, BTreeMap<String, String>>,
    volumes: Vec<Volume>,
    /// The five newest events about it (ScalingReplicaSet…), newest first.
    events: Vec<EventRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TemplateContainer {
    name: String,
    image: String,
    /// Its environment by name and source, never a value.
    env: Vec<Env>,
}

/// A variable, or with no name an `envFrom` that brings in every key of a
/// secret or configmap.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Env {
    name: Option<String>,
    /// The secret's id it reads: what `k8s secret get` takes.
    secret: Option<String>,
    /// The configmap's id it reads: what `k8s configmap get` takes.
    configmap: Option<String>,
    key: Option<String>,
}

fn deployment_get(ctx: &Ctx, args: DeploymentGetArgs) -> Result<DeploymentDetail> {
    let (target, name) = args.at.named(ctx, &args.deployment)?;
    let item = target.json(ctx, &["get", "deployment", &name, "-o", "json"])?;
    // ponytail: every ReplicaSet, pod and event of the namespace in one call;
    // `-l` with the deployment's selector if that grows to thousands.
    let listed = target.json(ctx, &["get", "replicasets,pods,events", "-o", "json"])?;
    let pods: Vec<&Value> = items(&listed)
        .filter(|held| held["kind"] == "Pod")
        .collect();
    let own = own_pods(&item, &pods);
    let row = row(&target, &item, &pods)
        .ok_or_else(|| anyhow::anyhow!("kubectl answered a deployment with no name"))?;
    let unavailable = item["status"]["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|held| held["type"] == "Available" && held["status"] == "False");
    if unavailable {
        let next = first_unready(&target, &own).unwrap_or_else(|| {
            format!(
                "agent-cli k8s event list --cluster {} --namespace {}",
                target.scope,
                target.namespace.as_deref().unwrap_or_default()
            )
        });
        ctx.note(format!("[next: {next}]"));
    }
    let template = &item["spec"]["template"]["spec"];
    let in_namespace = |held: &str| {
        let namespace = target.namespace.as_deref().unwrap_or_default();
        format!("{}/{namespace}/{held}", target.scope)
    };
    Ok(DeploymentDetail {
        row,
        strategy: strategy(&item["spec"]["strategy"]),
        conditions: conditions(&item),
        revisions: revisions(&listed, &name),
        containers: template_containers(template, &in_namespace),
        probes: probes(template),
        requests: sized(template, "requests"),
        limits: sized(template, "limits"),
        volumes: volumes(template, &in_namespace),
        events: events_in(&target, &listed, &item),
    })
}

fn strategy(strategy: &Value) -> Option<String> {
    let kind = strategy["type"].as_str()?;
    let rolling = &strategy["rollingUpdate"];
    if kind != "RollingUpdate" {
        return Some(kind.to_owned());
    }
    // The API fills in 25% for either left out.
    let bound = |key: &str| match &rolling[key] {
        Value::Null => "25%".to_owned(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Some(format!(
        "RollingUpdate (max surge {}, max unavailable {})",
        bound("maxSurge"),
        bound("maxUnavailable")
    ))
}

/// Each container's image and environment: a variable's name and the
/// secret or configmap key it comes from, an `envFrom`'s source; never a
/// value, which may be a secret written inline.
fn template_containers(
    spec: &Value,
    in_namespace: &dyn Fn(&str) -> String,
) -> Vec<TemplateContainer> {
    let source = |reference: &Value, name: Option<String>, key: Option<String>| {
        let secret = &reference["secretKeyRef"];
        let configmap = &reference["configMapKeyRef"];
        Env {
            name,
            secret: non_empty(&secret["name"]).map(in_namespace),
            configmap: non_empty(&configmap["name"]).map(in_namespace),
            key: key.or_else(|| {
                non_empty(&secret["key"])
                    .or_else(|| non_empty(&configmap["key"]))
                    .map(str::to_owned)
            }),
        }
    };
    spec["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|container| {
            let named = container["env"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|env| {
                    source(
                        &env["valueFrom"],
                        non_empty(&env["name"]).map(str::to_owned),
                        None,
                    )
                });
            let from = container["envFrom"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|from| Env {
                    name: None,
                    secret: non_empty(&from["secretRef"]["name"]).map(in_namespace),
                    configmap: non_empty(&from["configMapRef"]["name"]).map(in_namespace),
                    key: None,
                });
            Some(TemplateContainer {
                name: non_empty(&container["name"])?.to_owned(),
                image: non_empty(&container["image"])
                    .unwrap_or_default()
                    .to_owned(),
                env: named.chain(from).collect(),
            })
        })
        .collect()
}

command! {
    pub DEPLOYMENT_GET = ["k8s", "deployment", "get"], Read,
    "Describe a deployment: revision history, strategy, conditions, env, probes",
    keywords: ["revisions", "rollback", "roll", "back", "undo", "replicaset", "env", "variables", "available", "availability"],
    example: "k8s deployment get prod/web/api --fields revisions,conditions,containers",
    run: deployment_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn deployment_get_shows_its_revisions_newest_first_and_the_environment_by_name() {
        let outcome = run(&["k8s", "deployment", "get", "orders-api"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], "qa/dev/orders-api");
        assert_eq!(
            got["strategy"],
            "RollingUpdate (max surge 1, max unavailable 25%)"
        );
        assert_eq!(
            got["revisions"],
            json!([
                {"revision": 3, "images": ["contosoacr.azurecr.io/team/orders-api:1.2.3"],
                    "change_cause": "release 1.2.3", "created": "2026-09-10T07:58:00Z", "replicas": 2},
                {"revision": 2, "images": ["contosoacr.azurecr.io/team/orders-api:1.2.2"],
                    "created": "2026-08-20T10:00:00Z", "replicas": 0},
            ])
        );
        assert_eq!(
            got["containers"][0]["env"],
            json!([
                {"name": "DB_PASSWORD", "secret": "qa/dev/orders-db", "key": "password"},
                {"name": "DB_USER", "secret": "qa/dev/orders-db", "key": "username"},
                {"name": "LOG_LEVEL", "configmap": "qa/dev/orders-config", "key": "LOG_LEVEL"},
                {"secret": "qa/dev/orders-env"},
            ])
        );
        assert_eq!(got["requests"], json!({"api": {"cpu": "250m"}}));
        assert_eq!(got["events"][0]["reason"], "ScalingReplicaSet");
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);
    }

    #[test]
    fn a_deployment_that_is_not_available_names_its_unready_pod() {
        let outcome = run(&["k8s", "deployment", "get", "orders-worker"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["conditions"][0]["status"], "False");
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli k8s pod get qa/dev/orders-worker-5c4d3e-q8zt]"),
            "{}",
            outcome.stderr
        );
        let outcome = run(&["k8s", "deployment", "get", "gone"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli k8s deployment list"),
            "{}",
            outcome.stderr
        );
    }
}
