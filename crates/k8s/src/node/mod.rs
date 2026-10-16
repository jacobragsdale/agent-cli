//! `k8s node …`: a node as the scheduler sees it, what its pods request
//! against what it can give, which is what a Pending pod's "Insufficient
//! cpu" is about, and what metrics-server measured. `top` is shared with
//! `pod list`.

pub(crate) mod get;
pub(crate) mod list;

use std::collections::BTreeMap;

use agent_cli_core::Ctx;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{Target, age, non_empty};
use crate::pod::running;

#[derive(Debug, Serialize, JsonSchema)]
pub struct NodeRow {
    /// `cluster/name`: what `node get` takes.
    id: String,
    name: String,
    /// Its Ready condition holds; Unknown (the kubelet stopped posting) does not.
    ready: bool,
    /// False once cordoned: no new pod lands on it.
    schedulable: bool,
    /// The AKS node pool, from its agentpool label.
    pool: Option<String>,
    zone: Option<String>,
    /// The kubelet's version.
    version: Option<String>,
    /// `key=value:Effect`: what a pod must tolerate to land on it.
    taints: Vec<String>,
    cpu: Load,
    memory: Load,
    /// MemoryPressure, DiskPressure, PIDPressure or NetworkUnavailable, while it holds.
    pressure: Vec<String>,
    /// Pods on it that have not finished.
    pod_count: usize,
    age: Option<String>,
}

/// One resource of a node: requested and used against allocatable.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Load {
    /// Summed over the containers of its pods that have not finished: what
    /// the scheduler fits a new pod's requests against.
    requested: String,
    allocatable: Option<String>,
    /// Requested of allocatable, in percent.
    requested_percent: Option<u32>,
    /// What metrics-server measured (`kubectl top`), when it answered.
    usage: Option<String>,
    /// Used of allocatable, in percent.
    usage_percent: Option<u32>,
}

/// metrics-server's reading of one node or pod, as `kubectl top` prints it.
pub(crate) struct Used {
    pub(crate) cpu: String,
    pub(crate) memory: String,
    cpu_percent: Option<u32>,
    memory_percent: Option<u32>,
}

/// `kubectl top nodes|pods … --no-headers` by name (`namespace/name` for
/// pods across namespaces). None, with a note, when metrics-server does not
/// answer: usage is extra, never the command's failure.
pub(crate) fn top(ctx: &Ctx, target: &Target, args: &[&str]) -> Option<BTreeMap<String, Used>> {
    let mut argv = vec!["top"];
    argv.extend(args);
    argv.push("--no-headers");
    let text = match target.read(ctx, &argv) {
        Ok(text) => text,
        Err(error) => {
            ctx.note(format!("[usage not read: {error:#}]"));
            return None;
        }
    };
    let percent = |held: &str| held.strip_suffix('%')?.parse().ok();
    Some(
        text.lines()
            .filter_map(|line| {
                let cells: Vec<&str> = line.split_whitespace().collect();
                let (key, used) = match cells.as_slice() {
                    [name, cpu, cpu_percent, memory, memory_percent] => (
                        (*name).to_owned(),
                        Used {
                            cpu: (*cpu).to_owned(),
                            memory: (*memory).to_owned(),
                            cpu_percent: percent(cpu_percent),
                            memory_percent: percent(memory_percent),
                        },
                    ),
                    [namespace, name, cpu, memory] => {
                        (format!("{namespace}/{name}"), bare(cpu, memory))
                    }
                    [name, cpu, memory] => ((*name).to_owned(), bare(cpu, memory)),
                    _ => return None,
                };
                (used.cpu != "<unknown>").then_some((key, used))
            })
            .collect(),
    )
}

fn bare(cpu: &str, memory: &str) -> Used {
    Used {
        cpu: cpu.to_owned(),
        memory: memory.to_owned(),
        cpu_percent: None,
        memory_percent: None,
    }
}

/// Whether a pod still holds its requests on its node: every phase but
/// Succeeded and Failed, Pending ones bound to it included.
pub(crate) fn holds(pod: &Value) -> bool {
    !matches!(
        pod["status"]["phase"].as_str(),
        Some("Succeeded" | "Failed")
    )
}

/// What a pod requests, CPU in cores and memory in bytes, summed over the
/// containers that run.
// ponytail: kubectl describe takes the larger of that and the largest init
// container, and adds the pod's overhead; neither comes up on AKS's pods.
pub(crate) fn requests(pod: &Value) -> (f64, f64) {
    running(&pod["spec"]).fold((0.0, 0.0), |(cpu, memory), (_, container)| {
        let asked = &container["resources"]["requests"];
        let read = |key: &str| asked[key].as_str().and_then(quantity).unwrap_or(0.0);
        (cpu + read("cpu"), memory + read("memory"))
    })
}

/// A quantity (`250m`, `1.5`, `512Mi`, `13626184Ki`, `1G`) in its base
/// unit: cores, or bytes.
pub(crate) fn quantity(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    let at = raw
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(raw.len());
    let (number, suffix) = raw.split_at(at);
    let binary = |power: i32| 1024_f64.powi(power);
    let scale = match suffix {
        "" => 1.0,
        "n" => 1e-9,
        "u" => 1e-6,
        "m" => 1e-3,
        "k" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        "Ki" => binary(1),
        "Mi" => binary(2),
        "Gi" => binary(3),
        "Ti" => binary(4),
        _ => return None,
    };
    Some(number.parse::<f64>().ok()? * scale)
}

pub(crate) fn millicores(cores: f64) -> String {
    format!("{}m", (cores * 1000.0).round())
}

pub(crate) fn mebibytes(bytes: f64) -> String {
    format!("{}Mi", (bytes / 1024_f64.powi(2)).round())
}

/// The node's row. `pods` are every pod of the cluster, `used` its reading.
pub(crate) fn row(target: &Target, item: &Value, pods: &[&Value], used: Option<&Used>) -> NodeRow {
    let name = item["metadata"]["name"].as_str().unwrap_or_default();
    let labels = &item["metadata"]["labels"];
    let mine: Vec<&&Value> = pods
        .iter()
        .filter(|pod| pod["spec"]["nodeName"] == name && holds(pod))
        .collect();
    let (cpu, memory) = mine.iter().fold((0.0, 0.0), |(cpu, memory), pod| {
        let (asked_cpu, asked_memory) = requests(pod);
        (cpu + asked_cpu, memory + asked_memory)
    });
    let allocatable = &item["status"]["allocatable"];
    let load =
        |requested: f64, key: &str, show: fn(f64) -> String, used: Option<(&str, Option<u32>)>| {
            let can = allocatable[key].as_str().and_then(quantity);
            Load {
                requested: show(requested),
                allocatable: can.map(show),
                requested_percent: can
                    .filter(|can| *can > 0.0)
                    .map(|can| (requested / can * 100.0).round() as u32),
                usage: used.map(|(held, _)| held.to_owned()),
                usage_percent: used.and_then(|(_, percent)| percent),
            }
        };
    let conditions = item["status"]["conditions"]
        .as_array()
        .into_iter()
        .flatten();
    NodeRow {
        id: target.id(item),
        name: name.to_owned(),
        ready: conditions
            .clone()
            .any(|held| held["type"] == "Ready" && held["status"] == "True"),
        schedulable: item["spec"]["unschedulable"].as_bool() != Some(true),
        pool: non_empty(&labels["kubernetes.azure.com/agentpool"])
            .or_else(|| non_empty(&labels["agentpool"]))
            .map(str::to_owned),
        zone: non_empty(&labels["topology.kubernetes.io/zone"]).map(str::to_owned),
        version: non_empty(&item["status"]["nodeInfo"]["kubeletVersion"]).map(str::to_owned),
        taints: item["spec"]["taints"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|taint| {
                let key = taint["key"].as_str().unwrap_or_default();
                let effect = taint["effect"].as_str().unwrap_or_default();
                match non_empty(&taint["value"]) {
                    Some(value) => format!("{key}={value}:{effect}"),
                    None => format!("{key}:{effect}"),
                }
            })
            .collect(),
        cpu: load(
            cpu,
            "cpu",
            millicores,
            used.map(|held| (held.cpu.as_str(), held.cpu_percent)),
        ),
        memory: load(
            memory,
            "memory",
            mebibytes,
            used.map(|held| (held.memory.as_str(), held.memory_percent)),
        ),
        pressure: conditions
            .filter(|held| held["type"] != "Ready" && held["status"] == "True")
            .filter_map(|held| non_empty(&held["type"]).map(str::to_owned))
            .collect(),
        pod_count: mine.len(),
        age: age(&item["metadata"]["creationTimestamp"]),
    }
}

impl NodeRow {
    /// A node an agent should look at: not Ready, or under pressure.
    pub(crate) fn needs_attention(&self) -> bool {
        !self.ready || !self.pressure.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities_read_in_cores_and_bytes_whatever_their_suffix() {
        assert_eq!(quantity("250m"), Some(0.25));
        assert_eq!(quantity("1.5"), Some(1.5));
        assert_eq!(quantity("512Mi"), Some(512.0 * 1024.0 * 1024.0));
        assert_eq!(quantity("1G"), Some(1e9));
        assert_eq!(quantity("12x"), None);
        assert_eq!(millicores(3.7), "3700m");
        assert_eq!(mebibytes(13_953_212_416.0), "13307Mi");
    }
}
