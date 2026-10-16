use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, Target, items, non_empty};
use crate::pod::{status_word, strings};
use crate::shape::{Condition, EventRow, conditions, newest};

use super::{NodeRow, holds, mebibytes, millicores, requests, row, top, vm};

#[derive(clap::Args)]
pub struct NodeGetArgs {
    /// The node: its id (cluster/name) or name
    node: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NodeDetail {
    #[serde(flatten)]
    node: NodeRow,
    /// The Azure VM (a scale set instance) from its providerID: what
    /// `arm resource get` takes.
    vm: Option<String>,
    conditions: Vec<Condition>,
    /// Its InternalIP, and an ExternalIP if it has one.
    ips: Vec<String>,
    pod_cidr: Option<String>,
    capacity: BTreeMap<String, String>,
    allocatable: BTreeMap<String, String>,
    /// Its pods that have not finished, with what each requests.
    pods: Vec<NodePod>,
    /// The five newest events about it (NodeNotReady, NodeNotSchedulable…),
    /// newest first.
    events: Vec<EventRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NodePod {
    /// What `pod get` takes.
    id: String,
    status: String,
    cpu: String,
    memory: String,
}

fn node_get(ctx: &Ctx, args: NodeGetArgs) -> Result<NodeDetail> {
    let (target, name) = args.at.named_cluster(ctx, &args.node)?;
    let item = target.json(ctx, &["get", "node", &name, "-o", "json"])?;
    let selector = format!("spec.nodeName={name}");
    let listed = target.json(
        ctx,
        &[
            "get",
            "pods",
            "-o",
            "json",
            "--all-namespaces",
            "--field-selector",
            &selector,
        ],
    )?;
    let pods: Vec<&Value> = items(&listed).filter(|pod| holds(pod)).collect();
    let used = top(ctx, &target, &["node", &name]).unwrap_or_default();
    let node = row(&target, &item, &pods, used.get(&name));
    let vm = vm(&item).map(str::to_owned);
    if node.needs_attention() {
        let next = match (&node.nodepool, &vm) {
            (Some(pool), _) => format!("agent-cli aks nodepool get {pool}"),
            (None, Some(vm)) => format!("agent-cli arm resource get {vm}"),
            _ => format!(
                "agent-cli k8s event list --object node/{}",
                target.id(&item)
            ),
        };
        ctx.note(format!("[next: {next}]"));
    }
    let status = &item["status"];
    Ok(NodeDetail {
        vm,
        conditions: conditions(&item),
        ips: status["addresses"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|held| held["type"] != "Hostname")
            .filter_map(|held| non_empty(&held["address"]).map(str::to_owned))
            .collect(),
        pod_cidr: non_empty(&item["spec"]["podCIDR"]).map(str::to_owned),
        capacity: strings(&status["capacity"]),
        allocatable: strings(&status["allocatable"]),
        pods: pods
            .iter()
            .map(|pod| {
                let (cpu, memory) = requests(pod);
                NodePod {
                    id: format!(
                        "{}/{}/{}",
                        target.scope,
                        pod["metadata"]["namespace"].as_str().unwrap_or_default(),
                        pod["metadata"]["name"].as_str().unwrap_or_default()
                    ),
                    status: status_word(pod),
                    cpu: millicores(cpu),
                    memory: mebibytes(memory),
                }
            })
            .collect(),
        events: events(ctx, &target, &name),
        node,
    })
}

/// A node's events. The kubelet and node controller write them with the
/// node's name as their uid, so they are matched by kind and name, as
/// `kubectl describe node` matches them; they sit in `default`.
fn events(ctx: &Ctx, target: &Target, name: &str) -> Vec<EventRow> {
    let selector = format!("involvedObject.kind=Node,involvedObject.name={name}");
    let argv = [
        "get",
        "events",
        "-o",
        "json",
        "--all-namespaces",
        "--field-selector",
        &selector,
    ];
    match target.json(ctx, &argv) {
        Ok(listed) => newest(target, items(&listed)),
        Err(error) => {
            ctx.note(format!("[events not read: {error:#}]"));
            Vec::new()
        }
    }
}

command! {
    pub NODE_GET = ["k8s", "node", "get"], Read,
    "Show a node: conditions, requests, its Azure VM and AKS pool, events",
    keywords: ["notready", "cordoned", "pressure", "allocatable", "capacity", "vmss", "instance", "providerid", "agentpool", "kubelet"],
    example: "k8s node get prod/aks-user-27182818-vmss000001 --fields ready,conditions,vm,nodepool",
    run: node_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn a_pressured_node_names_its_vm_pool_pods_and_the_pool_to_read_next() {
        let outcome = k8s(&["k8s", "node", "get", "qa/aks-np1-vmss000000"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let node = outcome.json();
        assert_eq!(
            node["vm"],
            "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/mc_contoso-qa/providers/Microsoft.Compute/virtualMachineScaleSets/aks-np1-vmss/virtualMachines/0"
        );
        assert_eq!(node["nodepool"], "aks-qa/np1", "the context is the cluster");
        assert_eq!(node["ips"], json!(["10.224.0.4"]));
        assert_eq!(node["pod_cidr"], "10.244.0.0/24");
        assert_eq!(node["allocatable"]["cpu"], "1900m");
        assert_eq!(node["pressure"], json!(["DiskPressure"]));
        assert_eq!(
            node["pods"][0],
            json!({"id": "qa/dev/orders-api-7d9f5b-abc12", "status": "Running", "cpu": "250m", "memory": "256Mi"})
        );
        assert_eq!(
            node["pods"].as_array().unwrap().len(),
            9,
            "finished pods hold nothing"
        );
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli aks nodepool get aks-qa/np1]"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_not_ready_node_shows_its_conditions_and_events() {
        let outcome = k8s(&[
            "k8s",
            "node",
            "get",
            "aks-np1-vmss000001",
            "--cluster",
            "qa",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let node = outcome.json();
        assert_eq!(node["ready"], false);
        assert_eq!(node["conditions"][1]["status"], "Unknown");
        assert_eq!(node["events"][0]["reason"], "NodeNotReady");
        assert!(outcome.stderr.contains("[next: "), "{}", outcome.stderr);

        let outcome = k8s(&["k8s", "node", "get", "qa/gone"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("hint: agent-cli k8s node list"),
            "{}",
            outcome.stderr
        );
    }
}
