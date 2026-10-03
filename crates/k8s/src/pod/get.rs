use std::collections::{BTreeMap, BTreeSet};

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, Target, non_empty};

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
    let containers = containers(&item);
    if let Some(next) = previous_logs(&id, &containers) {
        ctx.note(format!("[next: {next}]"));
    }
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
        containers,
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
