use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, Target, items, non_empty};
use crate::pod::status_word;
use crate::shape::{EventRow, events_for, selects};

use super::{Endpoint, endpoints, external, kind, ready, selector};

/// What the Azure cloud provider reads off a service: an internal load
/// balancer, its subnet, a static IP, a DNS label, health probes.
const AZURE: &str = "service.beta.kubernetes.io/azure-";

#[derive(clap::Args)]
pub struct ServiceGetArgs {
    /// The service: its id (cluster/namespace/name), namespace/name, or name
    service: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceDetail {
    id: String,
    name: String,
    namespace: String,
    #[serde(rename = "type")]
    kind: String,
    cluster_ip: Option<String>,
    external: Vec<String>,
    ports: Vec<Port>,
    selector: Option<String>,
    /// Endpoints ready of all.
    ready: String,
    endpoints: Vec<Endpoint>,
    /// Why no endpoint is ready: no selector, a selector no pod matches, or
    /// pods that are not ready.
    why_none: Option<String>,
    /// The Azure load balancer annotations (internal, subnet, static IP…).
    azure: BTreeMap<String, String>,
    /// The five newest events about it, newest first.
    events: Vec<EventRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Port {
    name: Option<String>,
    port: i64,
    /// The pod's port it reaches, a named one resolved.
    target_port: Option<i64>,
    /// The container port's name, when the service names it.
    target_name: Option<String>,
    protocol: String,
    node_port: Option<i64>,
}

fn service_get(ctx: &Ctx, args: ServiceGetArgs) -> Result<ServiceDetail> {
    let (target, name) = args.at.named(ctx, &args.service)?;
    let item = target.json(ctx, &["get", "service", &name, "-o", "json"])?;
    // ponytail: reads every pod and slice in the namespace in one call; a
    // `-l` call per kind if a namespace holds thousands.
    let listed = target.json(ctx, &["get", "endpointslices,pods", "-o", "json"])?;
    let selector = selector(&item);
    let wanted = serde_json::json!({"matchLabels": item["spec"]["selector"]});
    let pods: Vec<&Value> = items(&listed)
        .filter(|pod| {
            pod["kind"] == "Pod"
                && selector.is_some()
                && selects(&wanted, &pod["metadata"]["labels"])
        })
        .collect();
    let endpoints = endpoints(&target, &listed, &item);
    let kind = kind(&item);
    let mut why_none = None;
    if kind != "ExternalName" && !endpoints.iter().any(|endpoint| endpoint.ready) {
        let (why, next) = no_endpoints(&target, selector.as_deref(), &pods);
        why_none = Some(why);
        if let Some(next) = next {
            ctx.note(format!("[next: {next}]"));
        }
    }
    Ok(ServiceDetail {
        id: target.id(&item),
        name,
        namespace: target.namespace.clone().unwrap_or_default(),
        kind,
        cluster_ip: non_empty(&item["spec"]["clusterIP"]).map(str::to_owned),
        external: external(&item),
        ports: item["spec"]["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|port| resolved(port, &listed, &item, &pods))
            .collect(),
        selector,
        ready: ready(&endpoints),
        endpoints,
        why_none,
        azure: item["metadata"]["annotations"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| key.starts_with(AZURE))
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect(),
        events: events_for(ctx, &target, &item),
    })
}

/// A port with a named target port resolved: from the service's slices,
/// which carry the number under the service port's name, else from the
/// container ports of the pods it selects.
fn resolved(port: &Value, listed: &Value, service: &Value, pods: &[&Value]) -> Port {
    let name = non_empty(&port["name"]);
    let number = port["port"].as_i64().unwrap_or_default();
    let target_name = non_empty(&port["targetPort"]);
    let target_port = match target_name {
        None => port["targetPort"].as_i64().or(Some(number)),
        Some(wanted) => {
            let sliced = items(listed)
                .filter(|item| {
                    item["kind"] == "EndpointSlice"
                        && item["metadata"]["labels"][super::SERVICE_NAME]
                            == service["metadata"]["name"]
                })
                .flat_map(|slice| slice["ports"].as_array().into_iter().flatten())
                .find(|held| non_empty(&held["name"]) == name)
                .and_then(|held| held["port"].as_i64());
            sliced.or_else(|| {
                pods.iter()
                    .flat_map(|pod| pod["spec"]["containers"].as_array().into_iter().flatten())
                    .flat_map(|container| container["ports"].as_array().into_iter().flatten())
                    .find(|held| held["name"].as_str() == Some(wanted))
                    .and_then(|held| held["containerPort"].as_i64())
            })
        }
    };
    Port {
        name: name.map(str::to_owned),
        port: number,
        target_port,
        target_name: target_name.map(str::to_owned),
        protocol: port["protocol"].as_str().unwrap_or("TCP").to_owned(),
        node_port: port["nodePort"].as_i64(),
    }
}

/// Why a service answers nothing, and the command that shows more: the
/// first unready pod's `pod get` (whose note leads on to its log), or the
/// namespace's events when no pod matches (a workload that cannot make any).
fn no_endpoints(
    target: &Target,
    selector: Option<&str>,
    pods: &[&Value],
) -> (String, Option<String>) {
    let namespace = target.namespace.as_deref().unwrap_or_default();
    let Some(selector) = selector else {
        return (
            "no selector: its endpoints are EndpointSlices made by hand, and none lists a ready address".to_owned(),
            None,
        );
    };
    if pods.is_empty() {
        return (
            format!("selector {selector} matches no pod in namespace {namespace}"),
            Some(format!(
                "agent-cli k8s event list --cluster {} --namespace {namespace}",
                target.scope
            )),
        );
    }
    let unready: Vec<&&Value> = pods.iter().filter(|pod| !crate::pod::ready(pod)).collect();
    let Some(first) = unready.first() else {
        return (
            format!(
                "the pods matching {selector} are ready, but no endpoint lists them: check the target port"
            ),
            None,
        );
    };
    let named: Vec<String> = unready
        .iter()
        .map(|pod| {
            format!(
                "{} ({})",
                pod["metadata"]["name"].as_str().unwrap_or_default(),
                status_word(pod)
            )
        })
        .collect();
    (
        format!("no pod matching {selector} is ready: {}", named.join(", ")),
        Some(format!("agent-cli k8s pod get {}", target.id(first))),
    )
}

command! {
    pub SERVICE_GET = ["k8s", "service", "get"], Read,
    "Show a Kubernetes service: ports, endpoint pods and IPs, or what leaves it empty",
    keywords: ["svc", "targetport", "unreachable", "backends", "behind", "empty"],
    example: "k8s service get qa/dev/orders-api --fields ports,ready,endpoints,why_none",
    run: service_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn a_service_resolves_its_named_target_port_and_lists_the_pods_behind_it() {
        let outcome = run(&["k8s", "service", "get", "orders-api"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let service = outcome.json();
        assert_eq!(
            service["ports"],
            json!([{"name": "http", "port": 80, "target_port": 8080, "target_name": "http", "protocol": "TCP"}])
        );
        assert_eq!(service["ready"], "2/2");
        assert_eq!(
            service["endpoints"][0],
            json!({"pod": "qa/dev/orders-api-7d9f5b-abc12", "ip": "10.244.0.7", "ready": true,
                "node": "aks-np1-vmss000000", "zone": "westeurope-1"})
        );
        assert!(service.get("why_none").is_none());
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);

        let billing = run(&["k8s", "service", "get", "qa/dev/billing-api"]).json();
        assert_eq!(
            billing["azure"],
            json!({"service.beta.kubernetes.io/azure-load-balancer-internal": "true"})
        );
        assert_eq!(billing["ports"][0]["node_port"], 31443);
    }

    #[test]
    fn a_service_with_no_ready_endpoint_says_why_and_names_the_next_command() {
        let outcome = run(&["k8s", "service", "get", "orders-worker"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["why_none"],
            "no pod matching app=orders-worker is ready: orders-worker-5c4d3e-q8zt (CrashLoopBackOff)"
        );
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli k8s pod get qa/dev/orders-worker-5c4d3e-q8zt]"),
            "{}",
            outcome.stderr
        );

        let outcome = run(&["k8s", "service", "get", "reports"]);
        let reports = outcome.json();
        assert_eq!(
            reports["why_none"],
            "selector app=reports matches no pod in namespace dev"
        );
        assert_eq!(
            reports["ports"],
            json!([{"port": 80, "target_name": "web", "protocol": "TCP"}]),
            "a name no pod has stays unresolved"
        );
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli k8s event list --cluster qa --namespace dev]"),
            "{}",
            outcome.stderr
        );

        let outcome = run(&["k8s", "service", "get", "legacy-db"]);
        assert!(
            outcome.json()["why_none"]
                .as_str()
                .unwrap()
                .starts_with("no selector"),
            "{outcome:?}"
        );
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);

        let outcome = run(&["k8s", "service", "get", "gone"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("hint: agent-cli k8s service list"),
            "{}",
            outcome.stderr
        );
    }
}
