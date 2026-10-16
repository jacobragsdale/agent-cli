//! `net vnet get`: the vnet's ARM GET joined to its `usages`, so each subnet
//! says how many IPs are used and free beside what is attached to it.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::text;
use crate::config::Azure;
use crate::net::vnet::{Peering, peering};
use crate::net::{API, Place, VNET, list, read, strings, usages};

#[derive(clap::Args)]
pub struct VnetGetArgs {
    /// The vnet: its ARM id, portal link, or name
    vnet: String,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct VnetRow {
    /// The ARM id, which `net vnet get` takes.
    id: String,
    name: String,
    location: Option<String>,
    address_space: Vec<String>,
    /// Empty: Azure's own DNS.
    dns_servers: Vec<String>,
    subnets: Vec<SubnetUse>,
    peerings: Vec<Peering>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SubnetUse {
    /// The subnet's ARM id, which `net subnet get` takes.
    id: String,
    name: String,
    prefix: Option<String>,
    used_ips: Option<u64>,
    /// What is left to hand out, after Azure's five reserved addresses.
    free_ips: Option<u64>,
    nsg: Option<String>,
    route_table: Option<String>,
    nat_gateway: Option<String>,
    /// The services the subnet is delegated to: Microsoft.Web/serverFarms …
    delegations: Vec<String>,
}

fn vnet_get(ctx: &Ctx, args: VnetGetArgs) -> Result<VnetRow> {
    let azure = Azure::load(ctx)?;
    let target = args.place.find(
        ctx,
        &azure,
        &args.vnet,
        &[VNET],
        "net vnet list",
        "net vnet get",
    )?;
    let vnet = read(ctx, &target.id, API, "net vnet list")?;
    let used = usages(ctx, &target.id)?;
    let props = &vnet["properties"];
    let subnets = list(&props["subnets"])
        .iter()
        .map(|subnet| {
            let held = &subnet["properties"];
            let id = text(&subnet["id"]).unwrap_or_default();
            let usage = used.iter().find(|(of, ..)| of.eq_ignore_ascii_case(&id));
            SubnetUse {
                name: text(&subnet["name"]).unwrap_or_default(),
                prefix: text(&held["addressPrefix"])
                    .or_else(|| Some(strings(&held["addressPrefixes"]).join(", ")))
                    .filter(|prefix| !prefix.is_empty()),
                used_ips: usage.map(|(_, used, _)| *used),
                free_ips: usage.map(|(_, _, free)| *free),
                nsg: text(&held["networkSecurityGroup"]["id"]),
                route_table: text(&held["routeTable"]["id"]),
                nat_gateway: text(&held["natGateway"]["id"]),
                delegations: list(&held["delegations"])
                    .iter()
                    .filter_map(|held| text(&held["properties"]["serviceName"]))
                    .collect(),
                id,
            }
        })
        .collect();
    Ok(VnetRow {
        id: text(&vnet["id"]).unwrap_or(target.id),
        name: text(&vnet["name"]).unwrap_or_default(),
        location: text(&vnet["location"]),
        address_space: strings(&props["addressSpace"]["addressPrefixes"]),
        dns_servers: strings(&props["dhcpOptions"]["dnsServers"]),
        subnets,
        peerings: list(&props["virtualNetworkPeerings"])
            .iter()
            .map(peering)
            .collect(),
    })
}

command! {
    pub VNET_GET = ["net", "vnet", "get"], Read,
    "Show a vnet's subnets with their used and free IPs, NSGs and route tables",
    keywords: ["virtual", "network", "usage", "capacity", "address", "exhausted", "cidr", "state"],
    example: "net vnet get vnet-contoso-prod --fields subnets",
    run: vnet_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RG, azure};

    #[test]
    fn each_subnet_gets_its_used_and_free_ips_from_the_vnets_usages() {
        let vnet = format!("{RG}/providers/Microsoft.Network/virtualNetworks/vnet-contoso");
        let aks = format!("{vnet}/subnets/aks");
        let nsg = format!("{RG}/providers/Microsoft.Network/networkSecurityGroups/nsg-aks");
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "vnet", "get", &vnet],
            vec![
                Answer::json(
                    &json!({"id": vnet, "name": "vnet-contoso", "location": "westeurope",
                    "properties": {"addressSpace": {"addressPrefixes": ["10.1.0.0/16"]},
                    "dhcpOptions": {"dnsServers": ["10.0.0.4"]},
                    "subnets": [
                        {"id": aks, "name": "aks", "properties": {"addressPrefix": "10.1.0.0/22",
                            "networkSecurityGroup": {"id": nsg}}},
                        {"id": format!("{vnet}/subnets/apps"), "name": "apps", "properties": {
                            "addressPrefixes": ["10.1.8.0/24"],
                            "delegations": [{"properties": {"serviceName": "Microsoft.Web/serverFarms"}}]}},
                    ],
                    "virtualNetworkPeerings": []}}),
                ),
                Answer::json(
                    &json!({"value": [{"id": aks.to_uppercase(), "currentValue": 1003,
                    "limit": 1019, "name": {"value": "Subnet size and usage"}, "unit": "Count"}]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["subnets"],
            json!([
                {"id": aks, "name": "aks", "prefix": "10.1.0.0/22", "used_ips": 1003,
                    "free_ips": 16, "nsg": nsg},
                {"id": format!("{vnet}/subnets/apps"), "name": "apps", "prefix": "10.1.8.0/24",
                    "delegations": ["Microsoft.Web/serverFarms"]},
            ])
        );
        assert_eq!(got["dns_servers"], json!(["10.0.0.4"]));
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            format!("https://management.azure.com{vnet}?api-version=2024-05-01")
        );
        assert_eq!(
            sent[1].url,
            format!("https://management.azure.com{vnet}/usages?api-version=2024-05-01")
        );
    }

    #[test]
    fn an_id_of_another_type_is_a_usage_error() {
        let nsg = format!("{RG}/providers/Microsoft.Network/networkSecurityGroups/nsg-aks");
        let (outcome, transport) = azure(&[crate::NET], &["net", "vnet", "get", &nsg], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }
}
