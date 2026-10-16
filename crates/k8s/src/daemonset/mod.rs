//! `k8s daemonset …`: a pod per node, read from kubectl's JSON. The
//! cluster's own (its CNI, CSI drivers, kube-proxy) run in kube-system, which
//! is read whatever namespaces the scope names, as `context get` reads it.

pub(crate) mod get;
pub(crate) mod list;

use std::collections::BTreeMap;

use agent_cli_core::Ctx;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, Target, age};
use crate::pod::{images, strings};

const SYSTEM: &str = "kube-system";

#[derive(Debug, Serialize, JsonSchema)]
pub struct DaemonSetRow {
    /// `cluster/namespace/name`: what `daemonset get` takes.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// Nodes that should run its pod.
    desired: i64,
    ready: i64,
    /// Pods on its current template.
    updated: i64,
    /// Nodes where its pod is missing or not ready.
    unavailable: i64,
    node_selector: BTreeMap<String, String>,
    images: Vec<String>,
    age: Option<String>,
}

fn row(target: &Target, item: &Value) -> Option<DaemonSetRow> {
    let status = &item["status"];
    let count = |key: &str| status[key].as_i64().unwrap_or(0);
    let template = &item["spec"]["template"]["spec"];
    Some(DaemonSetRow {
        id: target.id(item),
        name: item["metadata"]["name"].as_str()?.to_owned(),
        namespace: target.row_namespace(item),
        desired: count("desiredNumberScheduled"),
        ready: count("numberReady"),
        updated: count("updatedNumberScheduled"),
        unavailable: count("numberUnavailable"),
        node_selector: strings(&template["nodeSelector"]),
        images: images(template),
        age: age(&item["metadata"]["creationTimestamp"]),
    })
}

/// Where a listing reads: kube-system when `--namespace` names it, else
/// where every listing reads.
fn listing(ctx: &Ctx, at: &At) -> Result<Target> {
    if at.namespace.as_deref() == Some(SYSTEM) {
        return system(ctx, at.cluster.clone());
    }
    at.listing(ctx)
}

/// One daemonset by its ref, kube-system's whatever the scope names. A ref
/// and a flag that disagree go to `At::named`, which says so.
fn named(ctx: &Ctx, at: &At, raw: &str) -> Result<(Target, String)> {
    let parts: Vec<&str> = raw.trim().split('/').collect();
    let (cluster, namespace) = match parts.as_slice() {
        [cluster, namespace, _] => (Some(*cluster), Some(*namespace)),
        [namespace, _] => (None, Some(*namespace)),
        _ => (None, None),
    };
    let flag = at.namespace.as_deref();
    let agreed =
        |held: Option<&str>, flag: Option<&str>| held.zip(flag).is_none_or(|(a, b)| a == b);
    if namespace.or(flag) != Some(SYSTEM)
        || parts.len() > 3
        || parts.iter().any(|part| part.is_empty())
        || !agreed(namespace, flag)
        || !agreed(cluster, at.cluster.as_deref())
    {
        return at.named(ctx, raw);
    }
    let cluster = cluster.map(str::to_owned).or_else(|| at.cluster.clone());
    let name = parts.last().copied().unwrap_or_default();
    Ok((system(ctx, cluster)?, name.to_owned()))
}

fn system(ctx: &Ctx, cluster: Option<String>) -> Result<Target> {
    let target = At {
        cluster,
        namespace: None,
    }
    .cluster_wide(ctx)?;
    Ok(Target {
        namespace: Some(SYSTEM.to_owned()),
        cluster_scoped: false,
        ..target
    })
}
