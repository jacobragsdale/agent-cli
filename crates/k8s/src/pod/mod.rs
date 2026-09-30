//! `k8s pod …`: what a pod is, read from kubectl's JSON the way `kubectl get
//! pods` prints it. The STATUS word, the owner resolution (a ReplicaSet named
//! for a pod-template hash is its Deployment) and the container states are
//! ported from az-tui's `kube.rs`.

pub(crate) mod delete;
pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod logs;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{Target, age, digest, non_empty, owner_of};

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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{k8s, run};

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
}
