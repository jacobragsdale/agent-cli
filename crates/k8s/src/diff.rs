//! What the k8s diff commands share: both sides read by the ids rows print,
//! the noise every object carries, and a workload's pod template flattened
//! so paths read `containers.api.image`.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Secret, Side, diff, sensitive_key};
use anyhow::Result;
use serde_json::{Map, Value};

use crate::kubectl::{At, Target, non_empty};

/// Lists matched by name (or another field) rather than position.
const KEYS: &[(&str, &str)] = &[
    ("containers", "name"),
    ("initContainers", "name"),
    ("containers.*.env", "name"),
    ("initContainers.*.env", "name"),
    ("containers.*.ports", "containerPort"),
    ("containers.*.volumeMounts", "mountPath"),
    ("initContainers.*.volumeMounts", "mountPath"),
    ("volumes", "name"),
    ("imagePullSecrets", "name"),
    ("tolerations", "key"),
    ("spec.ports", "name"),
    ("spec.rules", "host"),
];

/// What every object carries that is bookkeeping, not configuration; a
/// cluster's own addresses too. Annotations and labels of that kind go in
/// [`clean`], since their keys hold dots.
const NOISE: &[&str] = &[
    "metadata.uid",
    "metadata.resourceVersion",
    "metadata.generation",
    "metadata.creationTimestamp",
    "metadata.managedFields",
    "metadata.selfLink",
    "metadata.ownerReferences",
    "metadata.name",
    "metadata.namespace",
    "status",
    "spec.clusterIP",
    "spec.clusterIPs",
];

/// The rules of every k8s diff.
pub(crate) const RULES: Rules = Rules {
    keys: KEYS,
    noise: NOISE,
    ..Rules::PLAIN
};

/// Labels and annotation prefixes the API server, kubectl and controllers
/// stamp; per object, never configuration.
const STAMPED: &[&str] = &[
    "kubectl.kubernetes.io/",
    "deployment.kubernetes.io/revision",
    "pod-template-hash",
    "controller-uid",
    "job-name",
    "batch.kubernetes.io/",
];

/// Two objects of one kind, each named as rows print them.
#[derive(clap::Args)]
pub struct Sides {
    /// One side: its id (cluster/namespace/name), namespace/name, or name
    left: String,
    /// The other side, as left
    right: String,
    #[command(flatten)]
    pub(crate) at: At,
    #[command(flatten)]
    pub(crate) diff: DiffArgs,
}

impl Sides {
    /// Both objects of `kind` as kubectl answers them, with their targets.
    pub(crate) fn read(&self, ctx: &Ctx, kind: &str) -> Result<[(Target, Value); 2]> {
        self.read_by(ctx, kind, |raw| self.at.named(ctx, raw))
    }

    /// As [`Sides::read`], each ref resolved by `named` (kube-system's
    /// daemonsets, a kind's own namespacing).
    pub(crate) fn read_by(
        &self,
        ctx: &Ctx,
        kind: &str,
        named: impl Fn(&str) -> Result<(Target, String)>,
    ) -> Result<[(Target, Value); 2]> {
        let one = |raw: &str| -> Result<(Target, Value)> {
            let (target, name) = named(raw)?;
            let item = target.json(ctx, &["get", kind, &name, "-o", "json"])?;
            Ok((target, item))
        };
        Ok([one(&self.left)?, one(&self.right)?])
    }

    /// Two workloads compared by [`workload`]; when they match, a note names
    /// the Secret both read.
    pub(crate) fn workloads(&self, ctx: &Ctx, sides: [(Target, Value); 2]) -> Result<Diff> {
        let [(lt, li), (rt, ri)] = sides;
        let (lrow, rrow) = (workload(&li, self.diff.all), workload(&ri, self.diff.all));
        if let Some(next) = next_secret((&lt, &lrow), (&rt, &rrow)) {
            ctx.note(next);
        }
        diff(
            ctx,
            &self.diff,
            &RULES,
            side(lt.id(&li), lrow),
            side(rt.id(&ri), rrow),
        )
    }

    /// Two objects compared as their manifests, cleaned.
    pub(crate) fn manifests(&self, ctx: &Ctx, sides: [(Target, Value); 2]) -> Result<Diff> {
        let [(lt, li), (rt, ri)] = sides;
        let (lid, rid) = (lt.id(&li), rt.id(&ri));
        let (l, r) = (clean(li, self.diff.all), clean(ri, self.diff.all));
        diff(
            ctx,
            &self.diff,
            &RULES,
            Side::new(lid, l),
            Side::new(rid, r),
        )
    }
}

/// A manifest without stamped labels and annotations (unless `--all`).
pub(crate) fn clean(mut item: Value, all: bool) -> Value {
    if !all && let Some(metadata) = item.get_mut("metadata") {
        unstamp(metadata);
    }
    crate::shape::trim(item)
}

fn unstamp(metadata: &mut Value) {
    for field in ["labels", "annotations"] {
        if let Some(map) = metadata.get_mut(field).and_then(Value::as_object_mut) {
            map.retain(|key, _| !STAMPED.iter().any(|stamp| key.starts_with(stamp)));
        }
    }
}

/// A workload as its diff compares it: the spec's own settings (replicas,
/// strategy; a CronJob's schedule and its Job's limits) beside its pod
/// template's labels, annotations and pod spec, at the top.
pub(crate) fn workload(item: &Value, all: bool) -> Value {
    let mut row = Map::new();
    let mut spec = &item["spec"];
    let own = |spec: &Value, row: &mut Map<String, Value>| {
        for (key, value) in spec.as_object().into_iter().flatten() {
            if !matches!(key.as_str(), "template" | "jobTemplate" | "selector") {
                row.insert(key.clone(), value.clone());
            }
        }
    };
    own(spec, &mut row);
    if spec.get("jobTemplate").is_some() {
        spec = &spec["jobTemplate"]["spec"];
        own(spec, &mut row);
    }
    row.extend(template(&spec["template"], all));
    Value::Object(row)
}

/// A pod template's labels, annotations and pod spec, as one map.
pub(crate) fn template(template: &Value, all: bool) -> Map<String, Value> {
    let mut metadata = template["metadata"].clone();
    if !all {
        unstamp(&mut metadata);
    }
    let mut row = Map::new();
    for field in ["labels", "annotations"] {
        if let Some(held) = metadata.get(field) {
            row.insert(field.to_owned(), held.clone());
        }
    }
    for (key, value) in template["spec"].as_object().into_iter().flatten() {
        row.insert(key.clone(), value.clone());
    }
    row
}

/// A workload row's inline environment values named like secrets
/// (`DB_PASSWORD`), taken out of the row: compared, never printed.
pub(crate) fn inline_secrets(row: &mut Value) -> BTreeMap<String, Option<Secret>> {
    let mut secrets = BTreeMap::new();
    for list in ["containers", "initContainers"] {
        for container in row[list].as_array_mut().into_iter().flatten() {
            let name = container["name"].as_str().unwrap_or_default().to_owned();
            for env in container["env"].as_array_mut().into_iter().flatten() {
                let variable = env["name"].as_str().unwrap_or_default().to_owned();
                if !sensitive_key(&variable) {
                    continue;
                }
                if let Some(Value::String(value)) =
                    env.as_object_mut().and_then(|e| e.remove("value"))
                {
                    secrets.insert(
                        format!("{list}.{name}.env.{variable}.value"),
                        Some(Secret::new(value)),
                    );
                }
            }
        }
    }
    secrets
}

/// A workload's side: its id, its row, its inline secrets.
pub(crate) fn side(id: String, mut row: Value) -> Side {
    let secrets = inline_secrets(&mut row);
    Side { id, row, secrets }
}

/// The Secrets a workload row reads (env `secretKeyRef`, `envFrom`), in
/// order.
fn secrets_read(row: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for container in row["containers"].as_array().into_iter().flatten() {
        let env = container["env"].as_array().into_iter().flatten();
        let from = env.filter_map(|env| non_empty(&env["valueFrom"]["secretKeyRef"]["name"]));
        let whole = container["envFrom"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|from| non_empty(&from["secretRef"]["name"]));
        for name in from.chain(whole) {
            if !names.iter().any(|held| held == name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// Where a workload diff goes next when the two match: the Secret both
/// read can still differ, so the first one each side reads.
pub(crate) fn next_secret(left: (&Target, &Value), right: (&Target, &Value)) -> Option<String> {
    let first = |(target, row): (&Target, &Value)| {
        let name = secrets_read(row).into_iter().next()?;
        let namespace = target.namespace.as_deref().unwrap_or_default();
        Some(format!("{}/{namespace}/{name}", target.scope))
    };
    Some(format!(
        "[next: agent-cli k8s secret diff {} {}]",
        first(left)?,
        first(right)?
    ))
}
