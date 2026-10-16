//! `net vnet list`: Resource Graph's virtual networks, with their subnets
//! and peerings from the same row, so one query answers what is peered with
//! what and which peering is down.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::text;
use crate::config::Azure;
use crate::net::vnet::{Peering, peering};
use crate::net::{Place, VNET, list, strings};

#[derive(clap::Args)]
pub struct VnetListArgs {
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct VnetListRow {
    /// The ARM id, which `net vnet get` takes.
    id: String,
    name: String,
    group: Option<String>,
    location: Option<String>,
    address_space: Vec<String>,
    subnets: Vec<Subnet>,
    peerings: Vec<Peering>,
    /// Empty: Azure's own DNS.
    dns_servers: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Subnet {
    /// `net subnet get VNET/NAME` takes it.
    name: String,
    prefix: Option<String>,
}

fn vnet_list(ctx: &Ctx, args: VnetListArgs) -> Result<Vec<VnetListRow>> {
    let azure = Azure::load(ctx)?;
    let project = "id, name, resourceGroup, location, \
        addressSpace = properties.addressSpace.addressPrefixes, subnets = properties.subnets, \
        peerings = properties.virtualNetworkPeerings, dnsServers = properties.dhcpOptions.dnsServers";
    Ok(args
        .place
        .list(ctx, &azure, VNET, project, args.limit)?
        .iter()
        .map(|row| VnetListRow {
            id: text(&row["id"]).unwrap_or_default(),
            name: text(&row["name"]).unwrap_or_default(),
            group: text(&row["resourceGroup"]),
            location: text(&row["location"]),
            address_space: strings(&row["addressSpace"]),
            subnets: list(&row["subnets"])
                .iter()
                .map(|subnet| Subnet {
                    name: text(&subnet["name"]).unwrap_or_default(),
                    prefix: text(&subnet["properties"]["addressPrefix"]),
                })
                .collect(),
            peerings: list(&row["peerings"]).iter().map(peering).collect(),
            dns_servers: strings(&row["dnsServers"]),
        })
        .collect())
}

command! {
    pub VNET_LIST = ["net", "vnet", "list"], Read,
    "List virtual networks with their address space, subnets, peerings and DNS",
    keywords: ["virtual", "network", "networks", "peered", "peering", "state", "cidr", "spoke", "hub"],
    example: "net vnet list --fields id,address_space,peerings",
    run: vnet_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{RG, azure, inventory};

    #[test]
    fn a_vnet_row_carries_its_subnets_and_peerings_from_resource_graph() {
        let vnet = format!("{RG}/providers/Microsoft.Network/virtualNetworks/vnet-contoso");
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "vnet", "list", "--group", "rg-contoso"],
            vec![inventory(vec![json!({
                "id": vnet, "name": "vnet-contoso", "resourceGroup": "rg-contoso",
                "location": "westeurope", "addressSpace": ["10.1.0.0/16"],
                "subnets": [{"name": "aks", "properties": {"addressPrefix": "10.1.0.0/22"}}],
                "peerings": [{"name": "to-hub", "properties": {
                    "remoteVirtualNetwork": {"id": "/subscriptions/s2/resourceGroups/rg-hub/providers/Microsoft.Network/virtualNetworks/vnet-hub"},
                    "peeringState": "Disconnected", "allowForwardedTraffic": true}}],
                "dnsServers": [],
            })])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": vnet, "name": "vnet-contoso", "group": "rg-contoso",
                "location": "westeurope", "address_space": ["10.1.0.0/16"],
                "subnets": [{"name": "aks", "prefix": "10.1.0.0/22"}],
                "peerings": [{"name": "to-hub", "state": "Disconnected", "forwarded_traffic": true,
                    "remote": "/subscriptions/s2/resourceGroups/rg-hub/providers/Microsoft.Network/virtualNetworks/vnet-hub"}]}])
        );
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert_eq!(
            query,
            "resources\n| where type =~ 'Microsoft.Network/virtualNetworks'\n| where resourceGroup =~ 'rg-contoso'\
             \n| project id, name, resourceGroup, location, addressSpace = properties.addressSpace.addressPrefixes, \
             subnets = properties.subnets, peerings = properties.virtualNetworkPeerings, \
             dnsServers = properties.dhcpOptions.dnsServers\n| order by name asc, id asc"
        );
    }
}
