use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, age, items, limited, non_empty};

use super::{endpoints, external, kind, ready, selector};

#[derive(clap::Args)]
pub struct ServiceListArgs {
    /// Part of the service name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceRow {
    /// `cluster/namespace/name`: what `service get` takes.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// ClusterIP, NodePort, LoadBalancer or ExternalName.
    #[serde(rename = "type")]
    kind: String,
    /// `None` for a headless service.
    cluster_ip: Option<String>,
    /// Load balancer IPs or host names, external IPs, an ExternalName's host.
    external: Vec<String>,
    /// `80→8080/TCP`; `443:31443→8443/TCP` with a node port.
    ports: Vec<String>,
    /// What it sends to: app=api, as `pod list --label` takes it.
    selector: Option<String>,
    /// Endpoints ready of all: `0/2` answers nothing.
    ready: Option<String>,
    age: Option<String>,
}

fn service_list(ctx: &Ctx, args: ServiceListArgs) -> Result<Vec<ServiceRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "services,endpointslices", "-o", "json"])?;
    let rows = items(&listed)
        .filter(|item| item["kind"] == "Service")
        .filter(|item| {
            args.name.as_deref().is_none_or(|part| {
                item["metadata"]["name"]
                    .as_str()
                    .is_some_and(|name| name.contains(part))
            })
        })
        .map(|item| ServiceRow {
            id: target.id(item),
            name: item["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            namespace: target.row_namespace(item),
            kind: kind(item),
            cluster_ip: non_empty(&item["spec"]["clusterIP"]).map(str::to_owned),
            external: external(item),
            ports: item["spec"]["ports"]
                .as_array()
                .into_iter()
                .flatten()
                .map(port)
                .collect(),
            selector: selector(item),
            ready: (kind(item) != "ExternalName")
                .then(|| ready(&endpoints(&target, &listed, item))),
            age: age(&item["metadata"]["creationTimestamp"]),
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

/// `80→8080/TCP`, as the port reaches the pod; a named target port stays a
/// name here (`service get` resolves it).
fn port(port: &Value) -> String {
    let number = port["port"].as_i64().unwrap_or_default();
    let node = port["nodePort"]
        .as_i64()
        .map_or_else(String::new, |node| format!(":{node}"));
    let target = match &port["targetPort"] {
        Value::Null => number.to_string(),
        Value::String(name) => name.clone(),
        other => other.to_string(),
    };
    let protocol = port["protocol"].as_str().unwrap_or("TCP");
    format!("{number}{node}→{target}/{protocol}")
}

command! {
    pub SERVICE_LIST = ["k8s", "service", "list"], Read,
    "List Kubernetes services: type, cluster IP, external IP, ports, ready endpoints",
    keywords: ["svc", "kubernetes", "loadbalancer", "clusterip", "ip", "nodeport", "exposed", "namespace", "all", "every"],
    example: "k8s service list --cluster qa --namespace dev --fields id,type,ports,ready",
    run: service_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn services_list_with_their_ports_and_how_many_endpoints_are_ready() {
        let outcome = run(&["k8s", "service", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"id": "qa/dev/orders-api", "name": "orders-api", "type": "ClusterIP",
                "cluster_ip": "10.0.10.20", "ports": ["80→http/TCP"], "selector": "app=orders-api",
                "ready": "2/2", "age": rows[0]["age"]})
        );
        assert_eq!(rows[1]["ready"], "0/1", "the crash-looping worker");
        assert_eq!(
            rows[2],
            json!({"id": "qa/dev/billing-api", "name": "billing-api", "type": "LoadBalancer",
                "cluster_ip": "10.0.10.22", "external": ["10.240.0.50"], "ports": ["443:31443→8443/TCP"],
                "selector": "app=billing-api", "ready": "1/1", "age": rows[2]["age"]})
        );
        assert!(rows[3].get("selector").is_none(), "{}", rows[3]);
        assert_eq!(rows[4]["ready"], "0/0");

        let outcome = run(&["k8s", "service", "list", "orders", "--fields", "id"]);
        assert_eq!(
            outcome.json(),
            json!([{"id": "qa/dev/orders-api"}, {"id": "qa/dev/orders-worker"}])
        );
    }
}
