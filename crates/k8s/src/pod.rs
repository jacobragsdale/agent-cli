//! `k8s pod …`: what a pod is, read from kubectl's JSON the way `kubectl get
//! pods` prints it. The STATUS word, the owner resolution (a ReplicaSet named
//! for a pod-template hash is its Deployment) and the container states are
//! ported from az-tui's `kube.rs`.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

use crate::{At, Target, age, items, limited, non_empty};

/// One pod as `kubectl get pods` lists it, with the owner resolved.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PodRow {
    /// `cluster/namespace/name`: what `pod get`, `logs` and `delete` take.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// kubectl's STATUS word: Running, CrashLoopBackOff, Init:1/2, Terminating…
    status: String,
    /// Containers ready of containers: 1/2.
    ready: String,
    restarts: u64,
    age: Option<String>,
    node: Option<String>,
    /// What made it: Deployment/orders-api, StatefulSet/redis, Job/…
    owner: Option<String>,
}

fn row(target: &Target, item: &Value) -> Option<PodRow> {
    let name = item["metadata"]["name"].as_str()?;
    let containers = containers(item);
    Some(PodRow {
        id: target.id(item),
        name: name.to_owned(),
        namespace: target.row_namespace(item),
        status: status_word(item),
        ready: format!(
            "{}/{}",
            containers.iter().filter(|held| held.ready).count(),
            containers.len()
        ),
        restarts: containers.iter().map(|held| held.restarts).sum(),
        age: age(&item["metadata"]["creationTimestamp"]),
        node: non_empty(&item["spec"]["nodeName"]).map(str::to_owned),
        owner: owner_of(item),
    })
}

/// One container, joined from its spec and its status.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ContainerRow {
    name: String,
    /// The image it runs, tag included: what `acr manifest get` takes.
    image: String,
    /// The digest it is running, when the kubelet reports one.
    digest: Option<String>,
    ready: bool,
    restarts: u64,
    /// Running, or why it waits or stopped: CrashLoopBackOff, Completed, ExitCode:137.
    state: String,
    /// Why it last stopped: "Error (exit 1)", "OOMKilled (exit 137)".
    last_termination: Option<String>,
}

fn containers(item: &Value) -> Vec<ContainerRow> {
    let statuses = item["status"]["containerStatuses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    item["spec"]["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|spec| {
            let name = spec["name"].as_str()?;
            let status = statuses
                .iter()
                .find(|status| status["name"].as_str() == Some(name));
            let state = status.map(|status| &status["state"]);
            let word = state.map_or_else(
                || "Waiting".to_owned(),
                |state| {
                    if !state["running"].is_null() {
                        "Running".to_owned()
                    } else if let Some(reason) = non_empty(&state["waiting"]["reason"]) {
                        reason.to_owned()
                    } else if !state["terminated"].is_null() {
                        termination_word(&state["terminated"])
                    } else {
                        "Waiting".to_owned()
                    }
                },
            );
            let last = status
                .map(|status| &status["lastState"]["terminated"])
                .filter(|terminated| !terminated.is_null())
                .map(|terminated| {
                    format!(
                        "{} (exit {})",
                        non_empty(&terminated["reason"]).unwrap_or("Terminated"),
                        terminated["exitCode"].as_i64().unwrap_or_default()
                    )
                });
            Some(ContainerRow {
                name: name.to_owned(),
                image: status
                    .and_then(|status| non_empty(&status["image"]))
                    .or_else(|| non_empty(&spec["image"]))
                    .unwrap_or_default()
                    .to_owned(),
                digest: status.and_then(|status| digest(&status["imageID"])),
                ready: status.is_some_and(|status| status["ready"].as_bool() == Some(true)),
                restarts: status
                    .and_then(|status| status["restartCount"].as_u64())
                    .unwrap_or_default(),
                state: word,
                last_termination: last,
            })
        })
        .collect()
}

/// What a stopped container says: its reason, or its exit code when it gave
/// none.
fn termination_word(terminated: &Value) -> String {
    non_empty(&terminated["reason"]).map_or_else(
        || {
            format!(
                "ExitCode:{}",
                terminated["exitCode"].as_i64().unwrap_or_default()
            )
        },
        str::to_owned,
    )
}

/// The STATUS word `kubectl get pods` prints, cut to the cases that come up:
/// the pod's own reason or phase, overridden by the first init container
/// still going, else by whatever the containers wait on or stopped for, and
/// `Terminating` over all of it once a delete is in.
// ponytail: skipped from kubectl's printPod — sidecar init containers,
// Signal:N, NotReady, NodeLost→Unknown, and the "(N ago)" restart suffix.
fn status_word(item: &Value) -> String {
    let status = &item["status"];
    let phase = status["phase"].as_str().unwrap_or("Unknown");
    let mut word = non_empty(&status["reason"]).unwrap_or(phase).to_owned();
    let init_total = item["spec"]["initContainers"]
        .as_array()
        .map_or(0, Vec::len);
    let mut initializing = false;
    for (index, held) in status["initContainerStatuses"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let state = &held["state"];
        let terminated = &state["terminated"];
        if !terminated.is_null() {
            if terminated["exitCode"].as_i64() == Some(0) {
                continue;
            }
            word = format!("Init:{}", termination_word(terminated));
        } else if let Some(reason) =
            non_empty(&state["waiting"]["reason"]).filter(|reason| *reason != "PodInitializing")
        {
            word = format!("Init:{reason}");
        } else {
            word = format!("Init:{index}/{init_total}");
        }
        initializing = true;
        break;
    }
    if !initializing {
        let mut has_running = false;
        // Back to front, the way kubectl reads them, so the first container's
        // reason is the one that stands.
        for held in status["containerStatuses"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
        {
            let state = &held["state"];
            if let Some(reason) = non_empty(&state["waiting"]["reason"]) {
                word = reason.to_owned();
            } else if !state["terminated"].is_null() {
                word = termination_word(&state["terminated"]);
            } else if held["ready"].as_bool() == Some(true) && !state["running"].is_null() {
                has_running = true;
            }
        }
        if word == "Completed" && has_running {
            word = "Running".to_owned();
        }
    }
    if !item["metadata"]["deletionTimestamp"].is_null() && !matches!(phase, "Succeeded" | "Failed")
    {
        word = "Terminating".to_owned();
    }
    word
}

/// The `sha256:…` a container status's `imageID` ends in
/// (`docker-pullable://…@sha256:…` or `…/api@sha256:…`).
pub(crate) fn digest(image_id: &Value) -> Option<String> {
    let (_, digest) = image_id.as_str()?.rsplit_once('@')?;
    (!digest.is_empty()).then(|| digest.to_owned())
}

/// What made the pod, as `Kind/name`. A ReplicaSet named after a
/// pod-template hash is a Deployment's, and is reported as that Deployment,
/// which is the name `k8s deployment restart` takes.
// ponytail: a Job's CronJob is not resolved; a ReplicaSet with no hash label
// stays a ReplicaSet.
pub(crate) fn owner_of(item: &Value) -> Option<String> {
    let references = item["metadata"]["ownerReferences"].as_array()?;
    let owner = references
        .iter()
        .find(|reference| reference["controller"].as_bool() == Some(true))
        .or_else(|| references.first())?;
    let kind = owner["kind"].as_str()?;
    let name = owner["name"].as_str()?;
    if kind == "ReplicaSet"
        && let Some(hash) = non_empty(&item["metadata"]["labels"]["pod-template-hash"])
        && let Some(base) = name.strip_suffix(&format!("-{hash}"))
    {
        return Some(format!("Deployment/{base}"));
    }
    Some(format!("{kind}/{name}"))
}

// ---------- k8s pod list ----------

#[derive(clap::Args)]
pub struct PodListArgs {
    /// Part of the pod name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    /// Only pods with this label: app=api, or a key alone (repeatable, all must hold)
    #[arg(long)]
    label: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn pod_list(ctx: &Ctx, args: PodListArgs) -> Result<Vec<PodRow>> {
    let target = args.at.listing(ctx)?;
    let selector = args.label.join(",");
    let mut argv = vec!["get", "pods", "-o", "json"];
    if !selector.is_empty() {
        argv.extend(["-l", selector.as_str()]);
    }
    let listed = target.json(ctx, &argv)?;
    let rows = items(&listed)
        .filter(|item| {
            args.name.as_deref().is_none_or(|part| {
                item["metadata"]["name"]
                    .as_str()
                    .is_some_and(|name| name.contains(part))
            })
        })
        .filter_map(|item| row(&target, item))
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub POD_LIST = ["k8s", "pod", "list"], Read,
    "List pods with status, ready, restarts, age, node and owning deployment",
    keywords: ["containers", "restarting", "crashloop", "crashing", "running", "pending", "unhealthy"],
    example: "k8s pod list --cluster qa --namespace dev --fields id,status,restarts,owner",
    run: pod_list,
}

// ---------- k8s pod get ----------

#[derive(clap::Args)]
pub struct PodGetArgs {
    /// The pod: its id (cluster/namespace/name), namespace/name, or name
    pod: String,
    #[command(flatten)]
    at: At,
    /// The manifest as YAML text instead
    #[arg(long)]
    yaml: bool,
}

/// A pod described: what `kubectl describe pod` says, as fields. With
/// `--yaml`, only the name, namespace and the manifest in `text`.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct PodDetail {
    id: String,
    name: String,
    namespace: String,
    status: Option<String>,
    ready: Option<String>,
    restarts: Option<u64>,
    age: Option<String>,
    node: Option<String>,
    ip: Option<String>,
    owner: Option<String>,
    containers: Vec<ContainerRow>,
    /// The secrets it reads: names and keys only, never values.
    secret_refs: Vec<SecretRef>,
    conditions: Vec<Condition>,
    labels: BTreeMap<String, String>,
    /// The manifest, with --yaml.
    text: Option<String>,
}

/// A secret a pod reads, by reference.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SecretRef {
    /// env, envFrom, volume, pull or csi.
    via: &'static str,
    /// The Kubernetes secret's id: what `k8s secret get` takes.
    secret: Option<String>,
    /// The keys it reads; none listed is every key.
    keys: Vec<String>,
    /// The SecretProviderClass, for csi.
    class: Option<String>,
    /// The Key Vault secrets the class mounts, as `kv secret get` takes them
    /// (`vault/name`).
    kv: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Condition {
    #[serde(rename = "type")]
    kind: String,
    status: String,
    reason: Option<String>,
    message: Option<String>,
}

fn pod_get(ctx: &Ctx, args: PodGetArgs) -> Result<PodDetail> {
    let (target, pod) = args.at.named(ctx, &args.pod)?;
    let namespace = target.namespace.clone().unwrap_or_default();
    let id = format!("{}/{namespace}/{pod}", target.scope);
    if args.yaml {
        return Ok(PodDetail {
            text: Some(target.read(ctx, &["get", "pod", &pod, "-o", "yaml"])?),
            id,
            name: pod,
            namespace,
            ..PodDetail::default()
        });
    }
    let item = target.json(ctx, &["get", "pod", &pod, "-o", "json"])?;
    let summary = row(&target, &item).unwrap_or_else(|| PodRow {
        id: id.clone(),
        name: pod.clone(),
        namespace: None,
        status: status_word(&item),
        ready: String::new(),
        restarts: 0,
        age: None,
        node: None,
        owner: None,
    });
    Ok(PodDetail {
        secret_refs: secret_refs(ctx, &target, &item),
        id,
        name: summary.name,
        namespace,
        status: Some(summary.status),
        ready: Some(summary.ready),
        restarts: Some(summary.restarts),
        age: summary.age,
        node: summary.node,
        ip: non_empty(&item["status"]["podIP"]).map(str::to_owned),
        owner: summary.owner,
        containers: containers(&item),
        conditions: item["status"]["conditions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|condition| {
                Some(Condition {
                    kind: non_empty(&condition["type"])?.to_owned(),
                    status: condition["status"].as_str().unwrap_or("Unknown").to_owned(),
                    reason: non_empty(&condition["reason"]).map(str::to_owned),
                    message: non_empty(&condition["message"]).map(str::to_owned),
                })
            })
            .collect(),
        labels: item["metadata"]["labels"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect(),
        text: None,
    })
}

command! {
    pub POD_GET = ["k8s", "pod", "get"], Read,
    "Describe a pod: containers, images, states, last termination reason, owner",
    keywords: ["describe", "crashloop", "why", "oomkilled", "image", "yaml", "manifest", "conditions", "vault", "uses"],
    example: "k8s pod get qa/dev/orders-api-7d9f5b-abc12 --fields status,containers,secret_refs",
    run: pod_get,
}

/// Every secret the pod's spec names: env and envFrom of every container,
/// secret volumes, image pull secrets, and CSI volumes with their
/// SecretProviderClass read (one kubectl call per class) to name the Key
/// Vault secrets behind them: the join an agent cannot cheaply do.
fn secret_refs(ctx: &Ctx, target: &Target, item: &Value) -> Vec<SecretRef> {
    let spec = &item["spec"];
    let id = |name: &str| {
        let namespace = target.namespace.as_deref().unwrap_or_default();
        format!("{}/{namespace}/{name}", target.scope)
    };
    // (via, secret) -> keys, in a stable order.
    let mut named: BTreeMap<(&'static str, String), BTreeSet<String>> = BTreeMap::new();
    let mut every = |via: &'static str, secret: &str, key: Option<&str>| {
        let keys = named.entry((via, secret.to_owned())).or_default();
        keys.extend(key.map(str::to_owned));
    };
    let containers = spec["containers"]
        .as_array()
        .into_iter()
        .chain(spec["initContainers"].as_array())
        .flatten();
    for container in containers {
        for env in container["env"].as_array().into_iter().flatten() {
            let reference = &env["valueFrom"]["secretKeyRef"];
            if let Some(secret) = non_empty(&reference["name"]) {
                every("env", secret, non_empty(&reference["key"]));
            }
        }
        for from in container["envFrom"].as_array().into_iter().flatten() {
            if let Some(secret) = non_empty(&from["secretRef"]["name"]) {
                every("envFrom", secret, None);
            }
        }
    }
    let mut classes = Vec::new();
    for volume in spec["volumes"].as_array().into_iter().flatten() {
        if let Some(secret) = non_empty(&volume["secret"]["secretName"]) {
            let items = volume["secret"]["items"].as_array();
            every("volume", secret, None);
            for item in items.into_iter().flatten() {
                every("volume", secret, non_empty(&item["key"]));
            }
        }
        let csi = &volume["csi"];
        if csi["driver"].as_str() == Some("secrets-store.csi.k8s.io")
            && let Some(class) = non_empty(&csi["volumeAttributes"]["secretProviderClass"])
        {
            classes.push(class.to_owned());
        }
    }
    for pull in spec["imagePullSecrets"].as_array().into_iter().flatten() {
        if let Some(secret) = non_empty(&pull["name"]) {
            every("pull", secret, None);
        }
    }
    let mut refs: Vec<SecretRef> = named
        .into_iter()
        .map(|((via, secret), keys)| SecretRef {
            via,
            secret: Some(id(&secret)),
            keys: keys.into_iter().collect(),
            class: None,
            kv: Vec::new(),
        })
        .collect();
    for class in classes {
        let kv = match target.json(ctx, &["get", "secretproviderclass", &class, "-o", "json"]) {
            Ok(found) => key_vault_ids(&found),
            Err(error) => {
                ctx.note(format!("[SecretProviderClass {class} not read: {error:#}]"));
                Vec::new()
            }
        };
        refs.push(SecretRef {
            via: "csi",
            secret: None,
            keys: Vec::new(),
            class: Some(class),
            kv,
        });
    }
    refs
}

/// The Key Vault secrets an Azure SecretProviderClass mounts, as `vault/name`:
/// `parameters.keyvaultName` and each `objectName` in the `objects` YAML
/// string whose `objectType` is a secret or a certificate (a certificate's
/// value is its backing secret); keys are not secrets.
fn key_vault_ids(class: &Value) -> Vec<String> {
    let parameters = &class["spec"]["parameters"];
    let Some(vault) = non_empty(&parameters["keyvaultName"]) else {
        return Vec::new();
    };
    let objects = parameters["objects"].as_str().unwrap_or_default();
    let field = |line: &str, key: &str| {
        line.trim()
            .trim_start_matches("- ")
            .trim()
            .strip_prefix(key)
            .map(|value| value.trim().trim_matches(['"', '\'']).to_owned())
    };
    let mut found: Vec<(String, String)> = Vec::new();
    for line in objects.lines() {
        if let Some(name) = field(line, "objectName:") {
            found.push((name, "secret".to_owned()));
        } else if let Some(kind) = field(line, "objectType:")
            && let Some(last) = found.last_mut()
        {
            last.1 = kind.to_ascii_lowercase();
        }
    }
    found
        .into_iter()
        .filter(|(name, kind)| !name.is_empty() && matches!(kind.as_str(), "secret" | "cert"))
        .map(|(name, _)| format!("{vault}/{name}"))
        .collect()
}

// ---------- k8s pod logs ----------

#[derive(clap::Args)]
pub struct PodLogsArgs {
    /// The pod: its id (cluster/namespace/name), namespace/name, or name
    pod: String,
    #[command(flatten)]
    at: At,
    /// Which container; kubectl picks the default one when left out
    #[arg(long)]
    container: Option<String>,
    /// How many of the last lines
    #[arg(long, default_value_t = 200)]
    tail: usize,
    /// The run before the last restart: where a crash loop says why
    #[arg(long)]
    previous: bool,
    /// Only lines logged after this
    #[arg(long)]
    since: Option<When>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Logs {
    /// The pod's id.
    pod: String,
    namespace: String,
    container: Option<String>,
    lines: usize,
    /// The log itself; cut from the front when it is long.
    text: String,
}

fn pod_logs(ctx: &Ctx, args: PodLogsArgs) -> Result<Logs> {
    let (target, pod) = args.at.named(ctx, &args.pod)?;
    let tail = format!("--tail={}", args.tail);
    let mut argv = vec!["logs", pod.as_str(), tail.as_str()];
    if let Some(container) = &args.container {
        argv.extend(["-c", container]);
    }
    if args.previous {
        argv.push("-p");
    }
    let since = args
        .since
        .map(|since| format!("--since-time={}", since.utc()));
    if let Some(since) = &since {
        argv.push(since);
    }
    let text = target.read(ctx, &argv)?;
    let namespace = target.namespace.unwrap_or_default();
    Ok(Logs {
        pod: format!("{}/{namespace}/{pod}", target.scope),
        namespace,
        container: args.container,
        lines: text.lines().count(),
        text,
    })
}

command! {
    pub POD_LOGS = ["k8s", "pod", "logs"], Read,
    "Read the tail of a pod's log, or of the container's previous run",
    keywords: ["log", "output", "stdout", "stderr", "tail", "crash", "error", "previous", "container"],
    example: "k8s pod logs qa/dev/orders-worker-5c4d3e-q8zt --previous --tail 50",
    run: pod_logs,
}

// ---------- k8s pod delete ----------

#[derive(clap::Args)]
pub struct PodDeleteArgs {
    /// The pod: its id (cluster/namespace/name), namespace/name, or name
    pod: String,
    #[command(flatten)]
    at: At,
}

/// What a change did, as kubectl said it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Changed {
    cluster: String,
    namespace: String,
    /// pod/NAME, deployment/NAME…
    object: String,
    /// kubectl's own line: `pod "x" deleted`.
    said: String,
    /// After a scale.
    replicas: Option<u32>,
    /// Before a scale.
    previous: Option<i64>,
}

fn pod_delete(ctx: &Ctx, args: PodDeleteArgs) -> Result<Changed> {
    let (target, pod) = args.at.named(ctx, &args.pod)?;
    // --wait=false: the pod goes Terminating and a controller replaces it;
    // waiting for the grace period would spend the deadline on nothing.
    let said = target.write(ctx, &["delete", "pod", &pod, "--wait=false"])?;
    Ok(Changed::new(
        &target,
        format!("pod/{pod}"),
        &said,
        None,
        None,
    ))
}

impl Changed {
    pub(crate) fn new(
        target: &Target,
        object: String,
        said: &str,
        replicas: Option<u32>,
        previous: Option<i64>,
    ) -> Self {
        Self {
            cluster: target.scope.clone(),
            namespace: target.namespace.clone().unwrap_or_default(),
            object,
            said: said.trim().to_owned(),
            replicas,
            previous,
        }
    }
}

command! {
    pub POD_DELETE = ["k8s", "pod", "delete"], Destructive,
    "Delete a pod so its controller replaces it (a bare pod is gone for good)",
    keywords: ["kill", "restart", "evict", "remove", "recreate"],
    example: "k8s pod delete orders-api-7d9f5b-abc12 --cluster qa --namespace dev",
    run: pod_delete,
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;
    use crate::fixtures::k8s;

    const DEV: [&str; 4] = ["--cluster", "qa", "--namespace", "dev"];

    fn with(argv: &[&str]) -> Vec<String> {
        argv.iter()
            .chain(DEV.iter())
            .map(|word| (*word).to_owned())
            .collect()
    }

    pub fn run(argv: &[&str]) -> agent_cli_core::testing::Outcome {
        let argv = with(argv);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        k8s(&argv)
    }

    #[test]
    fn pod_list_reads_status_ready_restarts_and_the_deployment_behind_a_replica_set() {
        let outcome = run(&["k8s", "pod", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 5);
        assert_eq!(rows[0]["name"], "orders-api-7d9f5b-abc12");
        assert_eq!(rows[0]["status"], "Running");
        assert_eq!(rows[0]["ready"], "1/1");
        assert_eq!(rows[0]["restarts"], 2);
        assert_eq!(rows[0]["owner"], "Deployment/orders-api");
        assert_eq!(rows[0]["node"], "aks-np1-vmss000000");
        assert!(
            rows[0]["age"].as_str().unwrap().ends_with('d')
                || rows[0]["age"].as_str().unwrap().ends_with("mo")
        );
        assert!(
            rows[0].get("namespace").is_none(),
            "one namespace needs no column"
        );
        assert_eq!(rows[2]["status"], "CrashLoopBackOff");
        assert_eq!(rows[2]["ready"], "0/1");
        assert_eq!(rows[3]["ready"], "2/2");
        assert_eq!(rows[4]["owner"], "StatefulSet/redis");

        let outcome = k8s(&[
            "k8s",
            "pod",
            "list",
            "--cluster",
            "all",
            "--fields",
            "name,namespace",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows.as_array().unwrap().len(),
            10,
            "every namespace of the qa context"
        );
        assert_eq!(rows[9]["namespace"], "uat");

        let outcome = run(&["k8s", "pod", "list", "worker", "--fields", "name"]);
        assert_eq!(
            outcome.json(),
            json!([{"name": "orders-worker-5c4d3e-q8zt"}])
        );
    }

    #[test]
    fn pod_list_selects_by_label() {
        let outcome = run(&[
            "k8s",
            "pod",
            "list",
            "--label",
            "app=billing-api",
            "--fields",
            "name",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "billing-api-1a2b3c-qq111"}])
        );
        let outcome = run(&[
            "k8s",
            "pod",
            "list",
            "--label",
            "app=orders-api",
            "--label",
            "pod-template-hash",
            "--fields",
            "name",
        ]);
        assert_eq!(outcome.json().as_array().unwrap().len(), 2, "{outcome:?}");
    }

    #[test]
    fn a_status_word_follows_kubectl_for_terminating_completed_and_init() {
        let outcome = k8s(&[
            "k8s",
            "pod",
            "list",
            "--cluster",
            "prod",
            "--fields",
            "name,status",
        ]);
        assert_eq!(outcome.json()[1]["status"], "Terminating");
        let outcome = k8s(&[
            "k8s",
            "pod",
            "list",
            "--cluster",
            "qa",
            "--namespace",
            "qa",
            "--fields",
            "status,owner",
        ]);
        assert_eq!(
            outcome.json()[2],
            json!({"status": "Completed", "owner": "Job/nightly-report"})
        );

        let init = json!({
            "metadata": {"name": "p"},
            "spec": {"initContainers": [{"name": "a"}, {"name": "b"}], "containers": [{"name": "c"}]},
            "status": {"phase": "Pending", "initContainerStatuses": [
                {"state": {"terminated": {"exitCode": 0}}},
                {"state": {"waiting": {"reason": "PodInitializing"}}},
            ]},
        });
        assert_eq!(status_word(&init), "Init:1/2");
        let failed = json!({"status": {"phase": "Pending", "initContainerStatuses": [{"state": {"terminated": {"exitCode": 3}}}]}});
        assert_eq!(status_word(&failed), "Init:ExitCode:3");
    }

    #[test]
    fn pod_get_describes_containers_with_the_last_termination() {
        let outcome = run(&["k8s", "pod", "get", "orders-worker-5c4d3e-q8zt"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let pod = outcome.json();
        assert_eq!(pod["status"], "CrashLoopBackOff");
        assert_eq!(pod["restarts"], 17);
        assert_eq!(pod["ip"], "10.244.0.7");
        assert_eq!(pod["owner"], "Deployment/orders-worker");
        assert_eq!(
            pod["containers"][0],
            json!({
                "name": "api", "image": "contosoacr.azurecr.io/team/orders-worker:1.2.3",
                "digest": "sha256:656e536a8d9d94acc380c0e37def06fffd0fcd9d8639b7d8caced4c537ab29b1",
                "ready": false, "restarts": 17, "state": "CrashLoopBackOff", "last_termination": "Error (exit 1)",
            })
        );
        assert_eq!(
            pod["conditions"][0],
            json!({"type": "Ready", "status": "False"})
        );
        assert_eq!(pod["labels"]["app"], "orders-worker");

        let outcome = run(&["k8s", "pod", "get", "redis-0", "--yaml"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let yaml = outcome.json();
        assert!(
            yaml["text"]
                .as_str()
                .unwrap()
                .starts_with("apiVersion: v1\nkind: Pod"),
            "{yaml}"
        );
        assert!(yaml.get("containers").is_none());

        let outcome = run(&["k8s", "pod", "get", "nope"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("pods \"nope\" not found"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn an_id_names_the_scope_and_namespace_and_a_disagreeing_flag_is_refused() {
        let outcome = k8s(&[
            "k8s",
            "pod",
            "get",
            "qa/dev/redis-0",
            "--fields",
            "id,name,namespace",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "qa/dev/redis-0", "name": "redis-0", "namespace": "dev"})
        );
        let outcome = k8s(&[
            "k8s",
            "pod",
            "get",
            "dev/redis-0",
            "--cluster",
            "aks-qa",
            "--fields",
            "id",
        ]);
        assert_eq!(
            outcome.code, 0,
            "a kube context names its scope: {outcome:?}"
        );
        assert_eq!(outcome.json(), json!({"id": "qa/dev/redis-0"}));
        let outcome = k8s(&["k8s", "pod", "get", "qa/dev/redis-0", "--namespace", "qa"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("qa/dev/redis-0 is in namespace dev, and --namespace says qa"),
            "{}",
            outcome.stderr
        );
        let listed = run(&["k8s", "pod", "list", "--fields", "id"]);
        assert_eq!(listed.json()[0]["id"], "qa/dev/orders-api-7d9f5b-abc12");
        let logs = k8s(&[
            "k8s",
            "pod",
            "logs",
            "qa/dev/redis-0",
            "--tail",
            "1",
            "--fields",
            "pod,lines",
        ]);
        assert_eq!(logs.json(), json!({"pod": "qa/dev/redis-0", "lines": 1}));
    }

    #[test]
    fn pod_get_names_every_secret_it_reads_and_the_key_vault_secrets_behind_its_csi_class() {
        let outcome = run(&[
            "k8s",
            "pod",
            "get",
            "orders-api-7d9f5b-abc12",
            "--fields",
            "secret_refs",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["secret_refs"],
            json!([
                {"via": "env", "secret": "qa/dev/orders-db", "keys": ["password", "username"]},
                {"via": "envFrom", "secret": "qa/dev/orders-env"},
                {"via": "pull", "secret": "qa/dev/acr-pull"},
                {"via": "volume", "secret": "qa/dev/orders-tls"},
                {"via": "csi", "class": "orders-kv", "kv": ["kv-contoso-dev/db-password", "kv-contoso-dev/api-cert"]},
            ])
        );
        assert!(
            !outcome.stdout.contains("aHVudGVyMg"),
            "names only, never values"
        );
        let other = run(&["k8s", "pod", "get", "redis-0", "--fields", "secret_refs"]);
        assert_eq!(other.stdout.trim(), "{}", "{other:?}");
    }

    #[test]
    fn logs_are_bounded_by_tail_and_can_read_the_previous_run() {
        let outcome = run(&[
            "k8s",
            "pod",
            "logs",
            "orders-worker-5c4d3e-q8zt",
            "--tail",
            "3",
            "--previous",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let logs = outcome.json();
        assert_eq!(logs["lines"], 3);
        let text = logs["text"].as_str().unwrap();
        assert!(text.ends_with("api previous line 30\n"), "{text}");
        assert!(text.starts_with("2026-09-12T12:00:28Z ERROR"), "{text}");

        let outcome = run(&[
            "k8s",
            "pod",
            "logs",
            "billing-api-1a2b3c-qq111",
            "--container",
            "proxy",
            "--tail",
            "1",
        ]);
        assert!(
            outcome.json()["text"]
                .as_str()
                .unwrap()
                .contains(" proxy line 30"),
            "{outcome:?}"
        );
    }

    #[test]
    fn delete_needs_yes_plans_under_dry_run_and_runs_without_waiting() {
        let outcome = run(&["k8s", "pod", "delete", "orders-api-7d9f5b-abc12"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("--yes"), "{}", outcome.stderr);

        let outcome = run(&[
            "k8s",
            "pod",
            "delete",
            "orders-api-7d9f5b-abc12",
            "--dry-run",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"dry_run": true, "would": [{"run": [
                "kubectl", "--context", "aks-qa", "--request-timeout=10s",
                "delete", "pod", "orders-api-7d9f5b-abc12", "--wait=false", "-n", "dev",
            ]}]})
        );

        let outcome = run(&["k8s", "pod", "delete", "orders-api-7d9f5b-abc12", "--yes"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"cluster": "qa", "namespace": "dev", "object": "pod/orders-api-7d9f5b-abc12",
                   "said": "pod \"orders-api-7d9f5b-abc12\" deleted"})
        );
    }
}
