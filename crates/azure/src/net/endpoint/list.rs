//! `net endpoint list`: Resource Graph's private endpoints, each joined to
//! its NIC's IPs in the same query, so a service's private address needs no
//! second call.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::text;
use crate::config::Azure;
use crate::net::endpoint::connection;
use crate::net::{ENDPOINT, Place, strings};

#[derive(clap::Args)]
pub struct EndpointListArgs {
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct EndpointListRow {
    /// The ARM id, which `net endpoint get` takes.
    id: String,
    name: String,
    group: Option<String>,
    /// The ARM id of what it reaches privately: a vault, a registry, a storage account.
    target: Option<String>,
    /// The sub-resources it reaches: `vault`, `registry`, `blob`.
    group_ids: Vec<String>,
    /// Approved, Pending, Rejected or Disconnected.
    status: Option<String>,
    /// The subnet's ARM id.
    subnet: Option<String>,
    ips: Vec<String>,
}

fn endpoint_list(ctx: &Ctx, args: EndpointListArgs) -> Result<Vec<EndpointListRow>> {
    let azure = Azure::load(ctx)?;
    let tail = "\n| extend nic = tolower(tostring(properties.networkInterfaces[0].id))\
        \n| join kind=leftouter (resources | where type =~ 'microsoft.network/networkinterfaces' \
        | mv-expand ip = properties.ipConfigurations \
        | summarize ips = make_list(tostring(ip.properties.privateIPAddress)) by nic = tolower(id)) on nic\
        \n| project id, name, resourceGroup, subnet = tostring(properties.subnet.id), \
        auto = properties.privateLinkServiceConnections, manual = properties.manualPrivateLinkServiceConnections, ips\
        \n| order by name asc, id asc";
    Ok(args
        .place
        .rows(ctx, &azure, &[ENDPOINT], tail, args.limit)?
        .iter()
        .map(|row| {
            let (target, group_ids, status) = connection(&row["auto"], &row["manual"]);
            EndpointListRow {
                id: text(&row["id"]).unwrap_or_default(),
                name: text(&row["name"]).unwrap_or_default(),
                group: text(&row["resourceGroup"]),
                target,
                group_ids,
                status,
                subnet: text(&row["subnet"]),
                ips: strings(&row["ips"]),
            }
        })
        .collect())
}

command! {
    pub ENDPOINT_LIST = ["net", "endpoint", "list"], Read,
    "List private endpoints with what they reach, their status and private IPs",
    keywords: ["private", "endpoints", "privatelink", "link", "approved", "pending", "address"],
    example: "net endpoint list --fields id,target,status,ips",
    run: endpoint_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{RG, azure, inventory};

    #[test]
    fn an_endpoint_row_names_its_target_and_its_nics_ips() {
        let id = format!("{RG}/providers/Microsoft.Network/privateEndpoints/pe-kv");
        let vault = format!("{RG}/providers/Microsoft.KeyVault/vaults/kv-contoso-dev");
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "endpoint", "list", "--group", "rg-contoso"],
            vec![inventory(vec![json!({"id": id, "name": "pe-kv",
                "resourceGroup": "rg-contoso", "subnet": "/x/subnets/pe", "auto": [],
                "manual": [{"name": "pe-kv", "properties": {"privateLinkServiceId": vault,
                    "groupIds": ["vault"], "privateLinkServiceConnectionState": {"status": "Pending"}}}],
                "ips": ["10.0.4.4"]})])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": id, "name": "pe-kv", "group": "rg-contoso", "target": vault,
                "group_ids": ["vault"], "status": "Pending", "subnet": "/x/subnets/pe",
                "ips": ["10.0.4.4"]}])
        );
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert!(
            query.as_str().unwrap().starts_with(
                "resources\n| where type =~ 'Microsoft.Network/privateEndpoints'\n| where resourceGroup =~ 'rg-contoso'\n| extend nic"
            ),
            "{query}"
        );
    }
}
