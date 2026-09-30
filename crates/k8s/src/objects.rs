//! Events, configmaps, secrets and workloads: az-tui's TUI-only kinds as
//! commands. A secret's listing carries key names and sizes only: kubectl
//! cannot leave the data out, so it is dropped here, before anything is
//! returned, and only `k8s secret get` decodes one key.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Failure, Secret, When, command};
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::pod::{Changed, digest, owner_of};
use crate::{At, Target, age, items, limited, non_empty};

// ---------- k8s event list ----------

#[derive(clap::Args)]
pub struct EventListArgs {
    #[command(flatten)]
    at: At,
    /// Only events about this pod: its id, namespace/name or name
    #[arg(long)]
    pod: Option<String>,
    /// Only events last seen after this
    #[arg(long)]
    since: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct EventRow {
    /// Normal or Warning.
    #[serde(rename = "type")]
    kind: String,
    reason: String,
    /// What it is about: Pod/orders-api-7d9f5b-abc12.
    object: String,
    message: String,
    count: i64,
    /// RFC 3339.
    last_seen: Option<String>,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
}

fn event_list(ctx: &Ctx, args: EventListArgs) -> Result<Vec<EventRow>> {
    // A pod's id names its namespace; a bare name keeps the listing's.
    let (target, pod) = match args.pod.as_deref() {
        Some(raw) if raw.contains('/') => {
            let (target, pod) = args.at.named(ctx, raw)?;
            (target, Some(pod))
        }
        other => (args.at.listing(ctx)?, other.map(str::to_owned)),
    };
    let listed = target.json(ctx, &["get", "events", "-o", "json"])?;
    let stamp = |value: &Value| {
        value
            .as_str()
            .and_then(|raw| OffsetDateTime::parse(raw, &Rfc3339).ok())
    };
    let mut rows: Vec<(Option<OffsetDateTime>, EventRow)> = items(&listed)
        .filter(|item| {
            pod.as_deref()
                .is_none_or(|pod| item["involvedObject"]["name"].as_str() == Some(pod))
        })
        .map(|item| {
            // Newer event APIs write eventTime and series in place of the
            // old stamps and count.
            let last = stamp(&item["lastTimestamp"])
                .or_else(|| stamp(&item["series"]["lastObservedTime"]))
                .or_else(|| stamp(&item["eventTime"]))
                .or_else(|| stamp(&item["metadata"]["creationTimestamp"]));
            let involved = &item["involvedObject"];
            let row = EventRow {
                kind: item["type"].as_str().unwrap_or("Normal").to_owned(),
                reason: item["reason"].as_str().unwrap_or_default().to_owned(),
                object: format!(
                    "{}/{}",
                    involved["kind"].as_str().unwrap_or_default(),
                    involved["name"].as_str().unwrap_or_default()
                ),
                message: item["message"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
                count: item["count"]
                    .as_i64()
                    .or_else(|| item["series"]["count"].as_i64())
                    .unwrap_or(1),
                last_seen: last.map(agent_cli_core::utc_time),
                namespace: target.row_namespace(item),
            };
            (last, row)
        })
        .collect();
    if let Some(since) = args.since {
        rows.retain(|(last, _)| last.is_some_and(|last| last >= since.0));
    }
    rows.sort_by_key(|(last, _)| std::cmp::Reverse(*last));
    Ok(limited(
        ctx,
        rows.into_iter().map(|(_, row)| row).collect(),
        args.limit,
    ))
}

command! {
    pub EVENT_LIST = ["k8s", "event", "list"], Read,
    "List Kubernetes events, newest first: warnings, back-offs, failed pulls",
    keywords: ["warning", "warnings", "backoff", "crashloop", "why", "failed", "pull", "scheduling", "oomkilled"],
    example: "k8s event list --pod qa/dev/orders-worker-5c4d3e-q8zt --since 1h --fields type,reason,message,last_seen",
    run: event_list,
}

// ---------- k8s configmap list / get ----------

#[derive(clap::Args)]
pub struct ConfigMapListArgs {
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ConfigMapRow {
    /// `cluster/namespace/name`: what `configmap get` takes.
    id: String,
    name: String,
    namespace: Option<String>,
    keys: Vec<String>,
    age: Option<String>,
}

fn configmap_list(ctx: &Ctx, args: ConfigMapListArgs) -> Result<Vec<ConfigMapRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "configmaps", "-o", "json"])?;
    let rows = items(&listed)
        .filter_map(|item| {
            Some(ConfigMapRow {
                id: target.id(item),
                name: item["metadata"]["name"].as_str()?.to_owned(),
                namespace: target.row_namespace(item),
                keys: data(item).into_keys().collect(),
                age: age(&item["metadata"]["creationTimestamp"]),
            })
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

/// A configmap's keys and values, a binary key's value being its size.
fn data(item: &Value) -> BTreeMap<String, String> {
    let mut data: BTreeMap<String, String> = item["data"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect();
    for (key, value) in item["binaryData"].as_object().into_iter().flatten() {
        let size = value.as_str().map_or(0, decoded_len);
        data.insert(key.clone(), format!("<binary, {size} bytes>"));
    }
    data
}

command! {
    pub CONFIGMAP_LIST = ["k8s", "configmap", "list"], Read,
    "List configmaps and their keys",
    keywords: ["config", "settings", "environment", "keys"],
    example: "k8s configmap list --cluster qa --namespace dev --fields id,keys",
    run: configmap_list,
}

#[derive(clap::Args)]
pub struct ConfigMapGetArgs {
    /// The configmap: its id (cluster/namespace/name), namespace/name, or name
    name: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ConfigMapDetail {
    id: String,
    name: String,
    namespace: String,
    age: Option<String>,
    /// Key to value; a binary key shows its size only.
    data: BTreeMap<String, String>,
}

fn configmap_get(ctx: &Ctx, args: ConfigMapGetArgs) -> Result<ConfigMapDetail> {
    let (target, name) = args.at.named(ctx, &args.name)?;
    let item = target.json(ctx, &["get", "configmap", &name, "-o", "json"])?;
    Ok(ConfigMapDetail {
        id: target.id(&item),
        name,
        namespace: target.namespace.unwrap_or_default(),
        age: age(&item["metadata"]["creationTimestamp"]),
        data: data(&item),
    })
}

command! {
    pub CONFIGMAP_GET = ["k8s", "configmap", "get"], Read,
    "Show a configmap's keys and values (binary keys show their size only)",
    keywords: ["config", "settings", "environment", "values", "variables", "hold", "contents"],
    example: "k8s configmap get qa/dev/orders-config",
    run: configmap_get,
}

// ---------- k8s secret list / get ----------

#[derive(clap::Args)]
pub struct SecretListArgs {
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A secret's shape. There is no field its data could go in.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SecretRow {
    /// `cluster/namespace/name`: what `secret get` takes.
    id: String,
    name: String,
    namespace: Option<String>,
    /// Opaque, kubernetes.io/tls, kubernetes.io/dockerconfigjson…
    #[serde(rename = "type")]
    kind: String,
    /// Key name to its decoded size in bytes.
    keys: BTreeMap<String, usize>,
    age: Option<String>,
}

fn secret_list(ctx: &Ctx, args: SecretListArgs) -> Result<Vec<SecretRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "secrets", "-o", "json"])?;
    let rows = items(&listed)
        .filter_map(|item| {
            Some(SecretRow {
                id: target.id(item),
                name: item["metadata"]["name"].as_str()?.to_owned(),
                namespace: target.row_namespace(item),
                kind: item["type"].as_str().unwrap_or("Opaque").to_owned(),
                keys: item["data"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(key, value)| (key.clone(), value.as_str().map_or(0, decoded_len)))
                    .collect(),
                age: age(&item["metadata"]["creationTimestamp"]),
            })
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub SECRET_LIST = ["k8s", "secret", "list"], Read,
    "List Kubernetes secrets: type, key names and sizes (never values)",
    keywords: ["password", "credential", "tls", "keys", "cluster"],
    example: "k8s secret list --cluster qa --namespace dev --fields id,type,keys",
    run: secret_list,
}

#[derive(clap::Args)]
pub struct SecretGetArgs {
    /// The secret: its id (cluster/namespace/name), namespace/name, or name
    name: String,
    /// Which key to decode
    key: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SecretValue {
    secret: String,
    key: String,
    namespace: String,
    value: String,
    /// "base64" when the bytes are not text, and `value` is left encoded.
    encoding: Option<String>,
}

fn secret_get(ctx: &Ctx, args: SecretGetArgs) -> Result<SecretValue> {
    let (target, name) = args.at.named(ctx, &args.name)?;
    let args = SecretGetArgs { name, ..args };
    let item = target.json(ctx, &["get", "secret", &args.name, "-o", "json"])?;
    let Some(encoded) = item["data"][&args.key].as_str() else {
        let keys: Vec<&str> = item["data"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, _)| key.as_str())
            .collect();
        return Err(Failure::not_found(format!(
            "secret {} has no key {:?}; its keys: {}",
            args.name,
            args.key,
            keys.join(", ")
        ))
        .into());
    };
    let bytes = base64_decode(encoded)
        .with_context(|| format!("{}/{} is not base64", args.name, args.key))?;
    let (secret, encoding) = match String::from_utf8(bytes) {
        Ok(text) => (Secret::new(text), None),
        Err(_) => (Secret::new(encoded), Some("base64".to_owned())),
    };
    Ok(SecretValue {
        secret: args.name,
        key: args.key,
        namespace: target.namespace.unwrap_or_default(),
        // The one place a Kubernetes secret's value leaves its Secret.
        value: secret.expose().to_owned(),
        encoding,
    })
}

command! {
    pub SECRET_GET = ["k8s", "secret", "get"], Reveal,
    "Decode one key of a Kubernetes secret (needs --reveal, or --output FILE)",
    keywords: ["password", "credential", "decode", "value", "cluster"],
    example: "k8s secret get orders-db password --cluster qa --namespace dev --fields value --output orders-db.txt",
    run: secret_get,
}

/// How many bytes a base64 string decodes to, without decoding it.
fn decoded_len(encoded: &str) -> usize {
    encoded.trim_end_matches('=').len() * 3 / 4
}

/// Standard base64, with or without padding, whitespace ignored.
// ponytail: twenty lines rather than a crate for the one decode.
fn base64_decode(encoded: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(encoded.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for character in encoded.bytes() {
        let value = match character {
            b'A'..=b'Z' => character - b'A',
            b'a'..=b'z' => character - b'a' + 26,
            b'0'..=b'9' => character - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            other => bail!("not base64: byte {other:#x}"),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Ok(bytes)
}

// ---------- k8s deployment list ----------

#[derive(clap::Args)]
pub struct DeploymentListArgs {
    /// Part of the deployment name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    /// Only deployments that rolled out after this
    #[arg(long)]
    since: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeploymentRow {
    /// `cluster/namespace/name`: what `deployment restart` and `scale` take.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// Pods ready of pods wanted: 2/3.
    ready: String,
    replicas: i64,
    images: Vec<Image>,
    /// When it last rolled out.
    updated: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Image {
    container: String,
    /// What the pod template asks for, tag included: what `acr manifest get`
    /// takes; the tag is the git tag that built it (`ado run list --branch`).
    image: String,
    /// The digest its pods run, when they report one.
    digest: Option<String>,
}

fn deployment_list(ctx: &Ctx, args: DeploymentListArgs) -> Result<Vec<DeploymentRow>> {
    let target = args.at.listing(ctx)?;
    // One call: the pods say which digest each tag resolved to.
    let listed = target.json(ctx, &["get", "deployments,pods", "-o", "json"])?;
    let (deployments, pods): (Vec<&Value>, Vec<&Value>) =
        items(&listed).partition(|item| item["kind"].as_str() == Some("Deployment"));
    let mut rows: Vec<(Option<OffsetDateTime>, DeploymentRow)> = deployments
        .into_iter()
        .filter_map(|item| {
            let name = item["metadata"]["name"].as_str()?;
            if !args.name.as_deref().is_none_or(|part| name.contains(part)) {
                return None;
            }
            let updated = rolled_out(item);
            let wanted = item["spec"]["replicas"].as_i64().unwrap_or(1);
            let namespace = item["metadata"]["namespace"].as_str();
            let own: Vec<&&Value> = pods
                .iter()
                .filter(|pod| {
                    pod["metadata"]["namespace"].as_str() == namespace
                        && owner_of(pod).as_deref() == Some(&format!("Deployment/{name}"))
                })
                .collect();
            let images = item["spec"]["template"]["spec"]["containers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|container| {
                    let name = non_empty(&container["name"])?;
                    let image = non_empty(&container["image"]).unwrap_or_default();
                    let digest = own
                        .iter()
                        .flat_map(|pod| {
                            pod["status"]["containerStatuses"]
                                .as_array()
                                .into_iter()
                                .flatten()
                        })
                        .filter(|status| status["name"].as_str() == Some(name))
                        .find_map(|status| digest(&status["imageID"]));
                    Some(Image {
                        container: name.to_owned(),
                        image: image.to_owned(),
                        digest,
                    })
                })
                .collect();
            let row = DeploymentRow {
                id: target.id(item),
                name: name.to_owned(),
                namespace: target.row_namespace(item),
                ready: format!(
                    "{}/{wanted}",
                    item["status"]["readyReplicas"].as_i64().unwrap_or(0)
                ),
                replicas: wanted,
                images,
                updated: updated.map(agent_cli_core::utc_time),
            };
            Some((updated, row))
        })
        .collect();
    if let Some(since) = args.since {
        rows.retain(|(updated, _)| updated.is_some_and(|at| at >= since.0));
    }
    Ok(limited(
        ctx,
        rows.into_iter().map(|(_, row)| row).collect(),
        args.limit,
    ))
}

/// When a deployment last rolled out: its Progressing condition's last
/// update, which moves with each new ReplicaSet, else when it was made.
fn rolled_out(item: &Value) -> Option<OffsetDateTime> {
    let stamp = |value: &Value| OffsetDateTime::parse(value.as_str()?, &Rfc3339).ok();
    item["status"]["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|condition| condition["type"].as_str() == Some("Progressing"))
        .and_then(|condition| stamp(&condition["lastUpdateTime"]))
        .or_else(|| stamp(&item["metadata"]["creationTimestamp"]))
}

command! {
    pub DEPLOYMENT_LIST = ["k8s", "deployment", "list"], Read,
    "List deployments: ready pods, images with tag and digest, when they rolled out",
    keywords: ["deployed", "running", "version", "image", "tag", "digest", "release", "rollout", "workloads", "prod", "production"],
    example: "k8s deployment list --cluster prod --namespace web --fields id,ready,images,updated",
    run: deployment_list,
}

// ---------- k8s deployment restart / scale ----------

/// A deployment's id (`cluster/namespace/name`) picks the scope and
/// namespace; `KIND/NAME` or a bare name keeps the flags'.
fn deployment_at(ctx: &Ctx, at: &At, raw: &str) -> Result<(Target, String)> {
    if raw.matches('/').count() == 2 {
        return at.named(ctx, raw);
    }
    Ok((at.one(ctx)?, raw.to_owned()))
}

/// The kinds `kubectl rollout restart` takes.
const ROLLABLE: &[&str] = &["deployment", "statefulset", "daemonset"];
/// The kinds `kubectl scale` takes.
const SCALABLE: &[&str] = &["deployment", "statefulset", "replicaset"];

/// `NAME` is a deployment; `KIND/NAME` names another workload, refused
/// unless `allowed` has it, as az-tui refused them.
fn workload(raw: &str, allowed: &[&str], done: &str) -> Result<String> {
    let (kind, name) = raw.split_once('/').unwrap_or(("deployment", raw));
    let kind = kind.to_ascii_lowercase();
    if name.is_empty() {
        return Err(Failure::usage(format!("{raw:?} names no workload")).into());
    }
    if !allowed.contains(&kind.as_str()) {
        return Err(Failure::usage(format!(
            "a {kind} cannot be {done}; only a {} can",
            allowed.join(", a ")
        ))
        .into());
    }
    Ok(format!("{kind}/{name}"))
}

#[derive(clap::Args)]
pub struct RestartArgs {
    /// A deployment's name or id (cluster/namespace/name), or statefulset/NAME, daemonset/NAME
    name: String,
    #[command(flatten)]
    at: At,
}

fn deployment_restart(ctx: &Ctx, args: RestartArgs) -> Result<Changed> {
    let (target, name) = deployment_at(ctx, &args.at, &args.name)?;
    let object = workload(&name, ROLLABLE, "restarted")?;
    let said = target.write(ctx, &["rollout", "restart", &object])?;
    Ok(Changed::new(&target, object, &said, None, None))
}

command! {
    pub DEPLOYMENT_RESTART = ["k8s", "deployment", "restart"], Destructive,
    "Rollout-restart a deployment, replacing its pods one at a time",
    keywords: ["rollout", "redeploy", "bounce", "recycle", "statefulset", "daemonset", "workload"],
    example: "k8s deployment restart orders-api --cluster qa --namespace dev",
    run: deployment_restart,
}

#[derive(clap::Args)]
pub struct ScaleArgs {
    /// A deployment's name or id (cluster/namespace/name), or statefulset/NAME, replicaset/NAME
    name: String,
    /// How many pods it should run
    #[arg(long)]
    replicas: u32,
    #[command(flatten)]
    at: At,
}

fn deployment_scale(ctx: &Ctx, args: ScaleArgs) -> Result<Changed> {
    let (target, name) = deployment_at(ctx, &args.at, &args.name)?;
    let object = workload(&name, SCALABLE, "scaled")?;
    // Read first: a name that is not there fails here with exit 4, and the
    // answer says what the count was.
    let before = target.json(ctx, &["get", &object, "-o", "json"])?;
    let previous = before["spec"]["replicas"].as_i64();
    let said = target.write(
        ctx,
        &["scale", &object, &format!("--replicas={}", args.replicas)],
    )?;
    Ok(Changed::new(
        &target,
        object,
        &said,
        Some(args.replicas),
        previous,
    ))
}

command! {
    pub DEPLOYMENT_SCALE = ["k8s", "deployment", "scale"], Destructive,
    "Scale a deployment to a number of replicas",
    keywords: ["replicas", "pods", "up", "down", "zero", "statefulset", "workload"],
    example: "k8s deployment scale orders-api --replicas 3 --cluster qa --namespace dev",
    run: deployment_scale,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::K8S;
    use crate::fixtures::k8s;
    use crate::pod::tests::run;

    #[test]
    fn the_json_of_a_listing_can_never_carry_a_value() {
        let outcome = run(&["k8s", "secret", "list", "--raw"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = &outcome.json()[0];
        assert_eq!(
            (&row["name"], &row["type"]),
            (&json!("orders-db"), &json!("Opaque"))
        );
        assert_eq!(
            row["keys"],
            json!({"password": 7, "username": 6}),
            "sizes, decoded"
        );
        for encoded in ["aHVudGVyMg==", "hunter2", "b3JkZXJz", "LS0tLS1CRUdJTg=="] {
            assert!(
                !outcome.stdout.contains(encoded),
                "{encoded}: {}",
                outcome.stdout
            );
        }
        let schema = serde_json::to_string(&(SECRET_LIST.returns)()).unwrap();
        assert!(
            !schema.contains("\"value\"") && !schema.contains("\"data\""),
            "{schema}"
        );

        let outcome = k8s(&["k8s", "secret", "list", "--cluster", "prod"]);
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(outcome.stderr.contains("(Forbidden)"), "{}", outcome.stderr);
    }

    #[test]
    fn secret_get_needs_reveal_decodes_one_key_and_names_the_keys_when_one_is_missing() {
        let outcome = run(&["k8s", "secret", "get", "orders-db", "password"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("--reveal"), "{}", outcome.stderr);

        let outcome = run(&["k8s", "secret", "get", "orders-db", "password", "--reveal"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"secret": "orders-db", "key": "password", "namespace": "dev", "value": "hunter2"})
        );

        let outcome = run(&["k8s", "secret", "get", "orders-db", "token", "--reveal"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("its keys: password, username"),
            "{}",
            outcome.stderr
        );
        let outcome = run(&["k8s", "secret", "get", "nope", "password", "--reveal"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }

    #[test]
    fn events_come_newest_first_and_narrow_to_one_pod() {
        let outcome = run(&["k8s", "event", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["reason"], "BackOff");
        assert_eq!(rows[0]["type"], "Warning");
        assert_eq!(rows[0]["count"], 17);
        assert_eq!(rows[0]["object"], "Pod/orders-worker-5c4d3e-q8zt");
        assert_eq!(rows[0]["last_seen"], "2026-09-12T12:30:00Z");
        assert_eq!(rows[2]["reason"], "ScalingReplicaSet");

        let outcome = run(&[
            "k8s",
            "event",
            "list",
            "--pod",
            "orders-api-7d9f5b-abc12",
            "--fields",
            "reason",
        ]);
        assert_eq!(outcome.json(), json!([{"reason": "Pulled"}]));
    }

    #[test]
    fn configmaps_list_their_keys_and_get_shows_values_with_binary_as_size() {
        let outcome = run(&["k8s", "configmap", "list", "--fields", "name,keys"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[0],
            json!({"name": "orders-config", "keys": ["DB_HOST", "FEATURES", "LOG_LEVEL"]})
        );

        let outcome = run(&[
            "k8s",
            "configmap",
            "get",
            "orders-config",
            "--fields",
            "data",
        ]);
        assert_eq!(outcome.json()["data"]["LOG_LEVEL"], "info");

        let binary = json!({"data": {"a": "x"}, "binaryData": {"blob": "AAECAwQF"}});
        assert_eq!(data(&binary)["blob"], "<binary, 6 bytes>");
        assert_eq!(base64_decode("aHVudGVyMg==").unwrap(), b"hunter2");
        assert!(base64_decode("not base64!").is_err());
    }

    #[test]
    fn restart_and_scale_refuse_kinds_they_cannot_act_on() {
        for (argv, want) in [
            (
                &["k8s", "deployment", "restart", "replicaset/x", "--yes"][..],
                "a replicaset cannot be restarted",
            ),
            (
                &[
                    "k8s",
                    "deployment",
                    "scale",
                    "daemonset/x",
                    "--replicas",
                    "2",
                    "--yes",
                ][..],
                "a daemonset cannot be scaled",
            ),
            (
                &[
                    "k8s",
                    "deployment",
                    "restart",
                    "job/nightly-report",
                    "--yes",
                ][..],
                "a job cannot be restarted",
            ),
        ] {
            let outcome = run(argv);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(outcome.stderr.contains(want), "{}", outcome.stderr);
        }
    }

    #[test]
    fn restart_and_scale_plan_under_dry_run_and_scale_reads_the_count_first() {
        let outcome = run(&["k8s", "deployment", "restart", "orders-api", "--dry-run"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["would"][0]["run"],
            json!([
                "kubectl",
                "--context",
                "aks-qa",
                "--request-timeout=10s",
                "rollout",
                "restart",
                "deployment/orders-api",
                "-n",
                "dev"
            ])
        );
        let outcome = run(&[
            "k8s",
            "deployment",
            "scale",
            "statefulset/redis",
            "--replicas",
            "0",
            "--dry-run",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["would"][0]["run"].as_array().unwrap()[4..],
            json!(["scale", "statefulset/redis", "--replicas=0", "-n", "dev"])
                .as_array()
                .unwrap()[..]
        );

        let outcome = run(&[
            "k8s",
            "deployment",
            "scale",
            "orders-api",
            "--replicas",
            "4",
            "--yes",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"cluster": "qa", "namespace": "dev", "object": "deployment/orders-api",
                   "said": "deployment/orders-api scaled", "replicas": 4, "previous": 2})
        );
        let outcome = run(&["k8s", "deployment", "restart", "orders-api", "--yes"]);
        assert_eq!(outcome.json()["said"], "deployment/orders-api restarted");
    }

    #[test]
    fn deployment_list_shows_ready_pods_images_with_digests_and_when_they_rolled_out() {
        let outcome = run(&["k8s", "deployment", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"id": "qa/dev/orders-api", "name": "orders-api", "ready": "2/2", "replicas": 2,
                "images": [{"container": "api", "image": "contosoacr.azurecr.io/team/orders-api:1.2.3",
                    "digest": "sha256:7f361af0fba5b2240abf4d78b24b30ae1ee06d320e9f0cdbabbcde359ae1a61b"}],
                "updated": "2026-09-10T08:00:00Z"})
        );
        assert_eq!(rows[1]["ready"], "0/1", "the crash-looping worker");
        assert_eq!(rows[2]["images"][1]["container"], "proxy");
        let names = |argv: &[&str]| {
            run(argv)
                .json()
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&["k8s", "deployment", "list", "billing"]),
            ["billing-api"]
        );
        assert!(names(&["k8s", "deployment", "list", "--since", "2026-09-11"]).is_empty());
        assert_eq!(
            names(&["k8s", "deployment", "list", "--since", "2026-09-09"]).len(),
            3
        );

        let everywhere = k8s(&[
            "k8s",
            "deployment",
            "list",
            "--cluster",
            "all",
            "--fields",
            "id,namespace",
        ]);
        assert_eq!(
            everywhere.json()[3],
            json!({"id": "all/qa/orders-api", "namespace": "qa"})
        );
    }

    #[test]
    fn events_narrow_to_a_pod_by_id_and_to_a_window() {
        let outcome = k8s(&[
            "k8s",
            "event",
            "list",
            "--pod",
            "qa/dev/orders-worker-5c4d3e-q8zt",
            "--fields",
            "reason,last_seen",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"reason": "BackOff", "last_seen": "2026-09-12T12:30:00Z"}])
        );
        let recent = run(&[
            "k8s",
            "event",
            "list",
            "--since",
            "2026-09-12T11:00:00Z",
            "--fields",
            "reason",
        ]);
        assert_eq!(
            recent.json(),
            json!([{"reason": "BackOff"}, {"reason": "Pulled"}])
        );
    }

    #[test]
    fn read_only_mode_refuses_every_change_before_kubectl_runs() {
        agent_cli_core::testing::assert_read_only_refuses(&[K8S]);
    }
}
