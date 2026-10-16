use std::collections::{BTreeMap, BTreeSet};

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, Target, non_empty};
use crate::shape::{Condition, EventRow, conditions, events_for};

use super::{
    ContainerRow, PodRow, containers, csi_classes, key_vault_ids, previous_logs, row, status_word,
};

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
    /// Every IP, both families on a dual-stack cluster.
    ips: Vec<String>,
    host_network: Option<bool>,
    /// ClusterFirst, Default (the node's resolver), None…
    dns_policy: Option<String>,
    /// The service account's id (cluster/namespace/name).
    service_account: Option<String>,
    owner: Option<String>,
    containers: Vec<ContainerRow>,
    ports: Vec<PortRow>,
    probes: Vec<Probe>,
    /// Container to its resource requests: `{"api": {"cpu": "250m"}}`.
    requests: BTreeMap<String, BTreeMap<String, String>>,
    /// Container to its resource limits.
    limits: BTreeMap<String, BTreeMap<String, String>>,
    volumes: Vec<Volume>,
    node_selector: BTreeMap<String, String>,
    /// `key=value:NoSchedule`, `key:NoExecute (300s)`; `*` tolerates every taint.
    tolerations: Vec<String>,
    /// The secrets it reads: names and keys only, never values.
    secret_refs: Vec<SecretRef>,
    conditions: Vec<Condition>,
    /// The five newest events about it, newest first.
    events: Vec<EventRow>,
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
pub struct PortRow {
    container: String,
    name: Option<String>,
    port: i64,
    protocol: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Probe {
    container: String,
    /// liveness, readiness or startup.
    kind: &'static str,
    /// `GET :http/healthz`, `TCP :8080`, `exec cat /tmp/ready`, `gRPC :9000`.
    action: String,
    initial_delay: Option<i64>,
    period: Option<i64>,
    failure_threshold: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Volume {
    name: String,
    /// persistentVolumeClaim, configMap, secret, csi, emptyDir, projected…
    kind: String,
    /// The PVC, configmap or secret's id; a CSI volume's driver and class.
    source: Option<String>,
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
    let containers = containers(&item);
    if let Some(next) = previous_logs(&id, &containers) {
        ctx.note(format!("[next: {next}]"));
    }
    let spec = &item["spec"];
    let in_namespace = |name: &str| format!("{}/{namespace}/{name}", target.scope);
    Ok(PodDetail {
        ips: item["status"]["podIPs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|held| non_empty(&held["ip"]).map(str::to_owned))
            .collect(),
        host_network: Some(spec["hostNetwork"].as_bool() == Some(true)),
        dns_policy: non_empty(&spec["dnsPolicy"]).map(str::to_owned),
        service_account: non_empty(&spec["serviceAccountName"]).map(in_namespace),
        ports: ports(spec),
        probes: probes(spec),
        requests: sized(spec, "requests"),
        limits: sized(spec, "limits"),
        volumes: volumes(spec, &in_namespace),
        node_selector: strings(&spec["nodeSelector"]),
        tolerations: spec["tolerations"]
            .as_array()
            .into_iter()
            .flatten()
            .map(toleration)
            .collect(),
        secret_refs: secret_refs(ctx, &target, &item),
        events: events_for(ctx, &target, &item),
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
        containers,
        conditions: conditions(&item),
        labels: strings(&item["metadata"]["labels"]),
        text: None,
    })
}

fn strings(map: &Value) -> BTreeMap<String, String> {
    map.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect()
}

/// The containers (and native sidecars) a pod runs, by name.
fn running(spec: &Value) -> impl Iterator<Item = (&str, &Value)> {
    spec["initContainers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|held| held["restartPolicy"] == "Always")
        .chain(spec["containers"].as_array().into_iter().flatten())
        .filter_map(|held| Some((non_empty(&held["name"])?, held)))
}

fn ports(spec: &Value) -> Vec<PortRow> {
    running(spec)
        .flat_map(|(container, held)| {
            held["ports"]
                .as_array()
                .into_iter()
                .flatten()
                .map(move |port| PortRow {
                    container: container.to_owned(),
                    name: non_empty(&port["name"]).map(str::to_owned),
                    port: port["containerPort"].as_i64().unwrap_or_default(),
                    protocol: port["protocol"].as_str().unwrap_or("TCP").to_owned(),
                })
        })
        .collect()
}

fn probes(spec: &Value) -> Vec<Probe> {
    let mut found = Vec::new();
    for (container, held) in running(spec) {
        for (key, kind) in [
            ("livenessProbe", "liveness"),
            ("readinessProbe", "readiness"),
            ("startupProbe", "startup"),
        ] {
            let probe = &held[key];
            if !probe.is_object() {
                continue;
            }
            let port = |handler: &Value| match &handler["port"] {
                Value::String(name) => name.clone(),
                other => other.to_string(),
            };
            let action = if probe["httpGet"].is_object() {
                let get = &probe["httpGet"];
                format!("GET :{}{}", port(get), get["path"].as_str().unwrap_or("/"))
            } else if probe["tcpSocket"].is_object() {
                format!("TCP :{}", port(&probe["tcpSocket"]))
            } else if probe["grpc"].is_object() {
                format!("gRPC :{}", port(&probe["grpc"]))
            } else {
                let command: Vec<&str> = probe["exec"]["command"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                format!("exec {}", command.join(" "))
            };
            found.push(Probe {
                container: container.to_owned(),
                kind,
                action,
                initial_delay: probe["initialDelaySeconds"].as_i64(),
                period: probe["periodSeconds"].as_i64(),
                failure_threshold: probe["failureThreshold"].as_i64(),
            });
        }
    }
    found
}

/// Each container's `resources.requests` or `.limits`, for those that set any.
fn sized(spec: &Value, key: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    running(spec)
        .map(|(container, held)| (container.to_owned(), strings(&held["resources"][key])))
        .filter(|(_, sizes)| !sizes.is_empty())
        .collect()
}

/// Each volume by the kind of its source; a PVC, configmap or secret by
/// its id, which its own command takes.
fn volumes(spec: &Value, in_namespace: &dyn Fn(&str) -> String) -> Vec<Volume> {
    spec["volumes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|volume| {
            let (kind, held) = volume
                .as_object()?
                .iter()
                .find(|(key, _)| key.as_str() != "name")?;
            let named = |key: &str| non_empty(&held[key]).map(in_namespace);
            let source = match kind.as_str() {
                "persistentVolumeClaim" => named("claimName"),
                "configMap" => named("name"),
                "secret" => named("secretName"),
                "csi" => non_empty(&held["driver"]).map(|driver| {
                    non_empty(&held["volumeAttributes"]["secretProviderClass"])
                        .map_or_else(|| driver.to_owned(), |class| format!("{driver} ({class})"))
                }),
                _ => None,
            };
            Some(Volume {
                name: volume["name"].as_str().unwrap_or_default().to_owned(),
                kind: kind.clone(),
                source,
            })
        })
        .collect()
}

/// A toleration as a taint reads: `key=value:Effect`, `key:Effect` for
/// Exists, `*` for an empty key with Exists, which tolerates every taint.
fn toleration(held: &Value) -> String {
    let key = held["key"].as_str().unwrap_or_default();
    let mut text = match (key, non_empty(&held["value"])) {
        ("", _) => "*".to_owned(),
        (key, Some(value)) if held["operator"] != "Exists" => format!("{key}={value}"),
        (key, _) => key.to_owned(),
    };
    if let Some(effect) = non_empty(&held["effect"]) {
        text = format!("{text}:{effect}");
    }
    if let Some(seconds) = held["tolerationSeconds"].as_i64() {
        text = format!("{text} ({seconds}s)");
    }
    text
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
    for volume in spec["volumes"].as_array().into_iter().flatten() {
        if let Some(secret) = non_empty(&volume["secret"]["secretName"]) {
            let items = volume["secret"]["items"].as_array();
            every("volume", secret, None);
            for item in items.into_iter().flatten() {
                every("volume", secret, non_empty(&item["key"]));
            }
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
    for class in csi_classes(item) {
        let kv = match target.json(ctx, &["get", "secretproviderclass", class, "-o", "json"]) {
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
            class: Some(class.to_owned()),
            kv,
        });
    }
    refs
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

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
        assert_eq!(
            pod["events"],
            json!([{"type": "Warning", "reason": "BackOff", "object": "Pod/orders-worker-5c4d3e-q8zt",
                "message": "Back-off restarting failed container api in pod orders-worker-5c4d3e-q8zt",
                "count": 17, "last_seen": "2026-09-12T12:30:00Z"}]),
            "its own events, matched by uid"
        );
        assert!(
            outcome.stderr.contains(
                "[next: agent-cli k8s pod logs qa/dev/orders-worker-5c4d3e-q8zt --previous --tail 50]"
            ),
            "{}",
            outcome.stderr
        );
        let healthy = run(&["k8s", "pod", "get", "redis-0"]);
        assert!(!healthy.stderr.contains("[next:"), "{}", healthy.stderr);

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
    fn pod_get_shows_how_it_runs_on_the_network_and_what_it_mounts() {
        let outcome = run(&[
            "k8s",
            "pod",
            "get",
            "orders-api-7d9f5b-abc12",
            "--fields",
            "ips,host_network,dns_policy,service_account,ports,probes,requests,limits,volumes,node_selector,tolerations",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({
                "ips": ["10.244.0.7", "fd00::7"],
                "host_network": false,
                "dns_policy": "ClusterFirst",
                "service_account": "qa/dev/orders-api",
                "ports": [{"container": "api", "name": "http", "port": 8080, "protocol": "TCP"}],
                "probes": [
                    {"container": "api", "kind": "liveness", "action": "TCP :8080", "initial_delay": 10, "failure_threshold": 3},
                    {"container": "api", "kind": "readiness", "action": "GET :http/healthz", "period": 5},
                ],
                "requests": {"api": {"cpu": "250m", "memory": "256Mi"}},
                "limits": {"api": {"memory": "512Mi"}},
                "volumes": [
                    {"name": "kv", "kind": "csi", "source": "secrets-store.csi.k8s.io (orders-kv)"},
                    {"name": "tls", "kind": "secret", "source": "qa/dev/orders-tls"},
                    {"name": "data", "kind": "persistentVolumeClaim", "source": "qa/dev/orders-data"},
                    {"name": "config", "kind": "configMap", "source": "qa/dev/orders-config"},
                    {"name": "kube-api-access-x1", "kind": "projected"},
                ],
                "node_selector": {"kubernetes.io/os": "linux"},
                "tolerations": ["node.kubernetes.io/not-ready:NoExecute (300s)", "pool=orders:NoSchedule"],
            })
        );
    }

    #[test]
    fn a_pod_that_is_not_there_is_exit_4_pointing_at_the_list() {
        let outcome = run(&["k8s", "pod", "get", "gone-pod"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("hint: agent-cli k8s pod list"),
            "{}",
            outcome.stderr
        );
    }
}
