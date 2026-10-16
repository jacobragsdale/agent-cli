//! `net lb get`: a load balancer's frontends, rules, probes and backend
//! pools, each frontend with its public address and the k8s service that
//! owns it: from the tag AKS puts on the public IP, else the service's uid,
//! which AKS writes into the frontend's name.

use std::collections::HashMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::arm::quote;
use crate::client::text;
use crate::config::Azure;
use crate::net::ip::public_ips;
use crate::net::{API, LB, Place, leaf, list, read};

#[derive(clap::Args)]
pub struct LbGetArgs {
    /// The load balancer: its ARM id, portal link, or name
    lb: String,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LbRow {
    /// The ARM id, which `net lb get` takes.
    id: String,
    name: String,
    group: Option<String>,
    sku: Option<String>,
    frontends: Vec<Frontend>,
    rules: Vec<Rule>,
    probes: Vec<Probe>,
    pools: Vec<Pool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Frontend {
    name: String,
    /// An internal frontend's address.
    private_ip: Option<String>,
    subnet: Option<String>,
    /// The public IP's ARM id.
    public_ip: Option<String>,
    /// The public IP's address.
    address: Option<String>,
    /// The k8s service it serves, `cluster/namespace/name` as `k8s service get` takes it.
    service: Option<String>,
    /// The service's uid, from the frontend's name, when no tag names it.
    service_uid: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Rule {
    name: String,
    frontend: Option<String>,
    pool: Option<String>,
    protocol: Option<String>,
    frontend_port: Option<u64>,
    backend_port: Option<u64>,
    probe: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Probe {
    name: String,
    protocol: Option<String>,
    port: Option<u64>,
    path: Option<String>,
    interval: Option<u64>,
    /// Failed probes before a backend is taken out.
    threshold: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Pool {
    name: String,
    /// NIC configurations (an AKS node each) or addresses in it.
    members: usize,
}

/// The uid AKS writes into a service's frontend name: `a`, the uid without
/// dashes, and a suffix for a frontend in another subnet.
fn uid(name: &str) -> Option<String> {
    let hex = name.strip_prefix('a')?;
    let (hex, rest) = hex.split_at_checked(32)?;
    if !hex.chars().all(|held| held.is_ascii_hexdigit())
        || !(rest.is_empty() || rest.starts_with('-'))
    {
        return None;
    }
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn lb_get(ctx: &Ctx, args: LbGetArgs) -> Result<LbRow> {
    let azure = Azure::load(ctx)?;
    let target = args
        .place
        .find(ctx, &azure, &args.lb, &[LB], "net lb list", "net lb get")?;
    let lb = read(ctx, &target.id, API, "net lb list")?;
    let props = &lb["properties"];
    let configs = list(&props["frontendIPConfigurations"]);
    let wanted: Vec<String> = configs
        .iter()
        .filter_map(|held| text(&held["properties"]["publicIPAddress"]["id"]))
        .map(|id| quote(&id))
        .collect();
    let ips: HashMap<String, _> = if wanted.is_empty() {
        HashMap::new()
    } else {
        let filter = format!("id in~ ({})", wanted.join(", "));
        public_ips(ctx, &azure, &Place::default(), Some(&filter), usize::MAX)?
            .into_iter()
            .map(|ip| (ip.id.to_ascii_lowercase(), ip))
            .collect()
    };
    let frontends = configs
        .iter()
        .map(|held| {
            let name = text(&held["name"]).unwrap_or_default();
            let fields = &held["properties"];
            let public_ip = text(&fields["publicIPAddress"]["id"]);
            let ip = public_ip
                .as_deref()
                .and_then(|id| ips.get(&id.to_ascii_lowercase()));
            let service = ip.and_then(|ip| ip.service.clone());
            Frontend {
                private_ip: text(&fields["privateIPAddress"]),
                subnet: text(&fields["subnet"]["id"]),
                address: ip.and_then(|ip| ip.address.clone()),
                service_uid: service.is_none().then(|| uid(&name)).flatten(),
                service,
                public_ip,
                name,
            }
        })
        .collect();
    let rules = list(&props["loadBalancingRules"])
        .iter()
        .map(|held| {
            let fields = &held["properties"];
            Rule {
                name: text(&held["name"]).unwrap_or_default(),
                frontend: leaf(&fields["frontendIPConfiguration"]["id"]),
                pool: leaf(&fields["backendAddressPool"]["id"]),
                protocol: text(&fields["protocol"]),
                frontend_port: fields["frontendPort"].as_u64(),
                backend_port: fields["backendPort"].as_u64(),
                probe: leaf(&fields["probe"]["id"]),
            }
        })
        .collect();
    let probes = list(&props["probes"])
        .iter()
        .map(|held| {
            let fields = &held["properties"];
            Probe {
                name: text(&held["name"]).unwrap_or_default(),
                protocol: text(&fields["protocol"]),
                port: fields["port"].as_u64(),
                path: text(&fields["requestPath"]),
                interval: fields["intervalInSeconds"].as_u64(),
                threshold: fields["probeThreshold"]
                    .as_u64()
                    .or(fields["numberOfProbes"].as_u64()),
            }
        })
        .collect();
    let pools = list(&props["backendAddressPools"])
        .iter()
        .map(|held| {
            let fields = &held["properties"];
            Pool {
                name: text(&held["name"]).unwrap_or_default(),
                // A NIC-based pool may list its members both ways.
                members: list(&fields["backendIPConfigurations"])
                    .len()
                    .max(list(&fields["loadBalancerBackendAddresses"]).len()),
            }
        })
        .collect();
    Ok(LbRow {
        id: target.id,
        name: text(&lb["name"]).unwrap_or_default(),
        group: target.group,
        sku: text(&lb["sku"]["name"]),
        frontends,
        rules,
        probes,
        pools,
    })
}

command! {
    pub LB_GET = ["net", "lb", "get"], Read,
    "Show a load balancer's frontends and their k8s services, rules, probes and pools",
    keywords: ["load", "balancer", "frontend", "backend", "health", "probe", "owns"],
    example: "net lb get kubernetes --group MC_rg-contoso-prod_aks-contoso-prod_westeurope --fields frontends",
    run: lb_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::uid;
    use crate::testing::{RG, azure, inventory};

    #[test]
    fn a_frontend_name_gives_the_service_uid_and_nothing_else_does() {
        assert_eq!(
            uid("a0f6b1c2d3e4f4a5b8c6d7e8f9a0b1c2d").as_deref(),
            Some("0f6b1c2d-3e4f-4a5b-8c6d-7e8f9a0b1c2d")
        );
        assert!(uid("a0f6b1c2d3e4f4a5b8c6d7e8f9a0b1c2d-aks").is_some());
        assert_eq!(uid("aksOutboundFrontend"), None);
        assert_eq!(uid("a0f6b1c2d3e4f4a5b8c6d7e8f9a0b1c2dx"), None);
    }

    #[test]
    fn frontends_name_their_service_from_the_public_ips_tag_or_their_own_name() {
        let id = format!("{RG}/providers/Microsoft.Network/loadBalancers/kubernetes");
        let ip = format!("{RG}/providers/Microsoft.Network/publicIPAddresses/kubernetes-a1");
        let frontend = |name: &str, public: Option<&str>| {
            json!({"id": format!("{id}/frontendIPConfigurations/{name}"), "name": name,
                "properties": {"publicIPAddress": public.map(|ip| json!({"id": ip})),
                    "privateIPAddress": public.map_or(json!("10.0.0.50"), |_| json!(null))}})
        };
        let internal = "a00112233445566778899aabbccddeeff";
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "lb", "get", &id],
            vec![
                Answer::json(
                    &json!({"id": id, "name": "kubernetes", "sku": {"name": "Standard"},
                    "properties": {
                    "frontendIPConfigurations": [frontend("a1", Some(&ip)), frontend(internal, None)],
                    "loadBalancingRules": [{"name": "a1-TCP-443", "properties": {
                        "frontendIPConfiguration": {"id": format!("{id}/frontendIPConfigurations/a1")},
                        "backendAddressPool": {"id": format!("{id}/backendAddressPools/kubernetes")},
                        "probe": {"id": format!("{id}/probes/a1-TCP-443")},
                        "protocol": "Tcp", "frontendPort": 443, "backendPort": 443}}],
                    "probes": [{"name": "a1-TCP-443", "properties": {"protocol": "Tcp", "port": 31443,
                        "intervalInSeconds": 5, "numberOfProbes": 2}}],
                    "backendAddressPools": [{"name": "kubernetes", "properties": {
                        "backendIPConfigurations": [{"id": "/n/1"}, {"id": "/n/2"}]}}]}}),
                ),
                inventory(vec![
                    json!({"id": ip, "name": "kubernetes-a1", "address": "203.0.113.10",
                    "service": "web/nginx", "cluster": "aks-contoso-dev"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["frontends"],
            json!([{"name": "a1", "public_ip": ip, "address": "203.0.113.10",
                "service": "aks-contoso-dev/web/nginx"},
                {"name": internal, "private_ip": "10.0.0.50",
                "service_uid": "00112233-4455-6677-8899-aabbccddeeff"}])
        );
        assert_eq!(
            got["rules"],
            json!([{"name": "a1-TCP-443", "frontend": "a1", "pool": "kubernetes", "protocol": "Tcp",
                "frontend_port": 443, "backend_port": 443, "probe": "a1-TCP-443"}])
        );
        assert_eq!(got["probes"][0]["threshold"], 2);
        assert_eq!(got["pools"], json!([{"name": "kubernetes", "members": 2}]));
        let query = transport.sent()[1].body.clone().unwrap()["query"].clone();
        assert!(
            query
                .as_str()
                .unwrap()
                .contains(&format!("| where id in~ ('{ip}')")),
            "{query}"
        );
    }
}
