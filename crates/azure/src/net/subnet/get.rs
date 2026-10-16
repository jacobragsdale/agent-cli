//! `net subnet get`: the subnet's ARM GET, its vnet's `usages` for used and
//! free IPs, and Resource Graph for who holds them, which is what "the
//! subnet is full" needs next: is it the AKS pools, private endpoints or VMs.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::arm::quote;
use crate::arm::resource::target;
use crate::client::text;
use crate::config::Azure;
use crate::graph::rows;
use crate::net::{API, Place, SUBNET, VNET, ids, list, read, strings, usages};

#[derive(clap::Args)]
pub struct SubnetGetArgs {
    /// The subnet: its ARM id, portal link, or VNET/SUBNET
    subnet: String,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SubnetRow {
    /// The ARM id, which `net subnet get` takes.
    id: String,
    name: String,
    /// The vnet's ARM id.
    vnet: String,
    prefix: Option<String>,
    used_ips: Option<u64>,
    /// What is left to hand out, after Azure's five reserved addresses.
    free_ips: Option<u64>,
    nsg: Option<String>,
    route_table: Option<String>,
    nat_gateway: Option<String>,
    service_endpoints: Vec<String>,
    delegations: Vec<String>,
    private_endpoints: Vec<String>,
    /// Who holds its IPs, most first.
    holders: Vec<Holder>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Holder {
    /// The holder's ARM id: a VM, private endpoint, NIC or scale set.
    id: String,
    /// Its ARM type.
    #[serde(rename = "type")]
    kind: String,
    ips: u64,
    /// For an AKS node pool's scale set, `cluster/pool`, which `aks nodepool get` takes.
    pool: Option<String>,
}

/// The subnet's ARM id: an id or link as it is, or `VNET/SUBNET` with the
/// vnet found by name.
fn subnet_id(ctx: &Ctx, azure: &Azure, args: &SubnetGetArgs) -> Result<String> {
    let raw = args.subnet.trim();
    let id_like = raw.starts_with('/')
        || raw.starts_with("https://")
        || raw.to_ascii_lowercase().starts_with("subscriptions/");
    let get = "net subnet get";
    if id_like {
        let found = args
            .place
            .find(ctx, azure, raw, &[SUBNET], "net vnet list", get)?;
        return Ok(found.id);
    }
    let Some((vnet, subnet)) = raw
        .split_once('/')
        .filter(|(_, subnet)| !subnet.is_empty() && !subnet.contains('/'))
    else {
        return Err(Failure::usage(format!(
            "{raw:?} is not a subnet's ARM id, portal link or VNET/SUBNET"
        ))
        .hint("the vnets and their subnets: agent-cli net vnet list --fields id,subnets")
        .into());
    };
    let vnet = args
        .place
        .find(ctx, azure, vnet, &[VNET], "net vnet list", "net vnet get")?;
    Ok(format!("{}/subnets/{subnet}", vnet.id))
}

/// Who holds IPs in `subnet`, by Resource Graph: NICs (their VM or private
/// endpoint, else the NIC itself), and scale sets, an AKS pool among them,
/// whose instances' NICs are not resources of their own.
// ponytail: app gateways, load balancer frontends, firewalls and gateways
// hold IPs too and are not looked up; the note says when the holders found
// fall short of the IPs in use.
fn holders(ctx: &Ctx, azure: &Azure, subnet: &str) -> Result<Vec<Holder>> {
    let subnet = quote(subnet);
    let nics = format!(
        "resources\n| where type =~ 'microsoft.network/networkinterfaces'\
         \n| mv-expand ip = properties.ipConfigurations\
         \n| where tostring(ip.properties.subnet.id) =~ {subnet}\
         \n| summarize ips = count() by holder = coalesce(tostring(properties.privateEndpoint.id), tostring(properties.virtualMachine.id), id)\
         \n| order by holder asc"
    );
    // VERIFY(work): that AKS tags its scale sets aks-managed-poolName.
    let sets = format!(
        "resources\n| where type =~ 'microsoft.compute/virtualmachinescalesets'\
         \n| mv-expand nic = properties.virtualMachineProfile.networkProfile.networkInterfaceConfigurations\
         \n| mv-expand ip = nic.properties.ipConfigurations\
         \n| where tostring(ip.properties.subnet.id) =~ {subnet}\
         \n| summarize per_node = count() by holder = id, nodes = toint(sku.capacity), \
         pool = tostring(tags['aks-managed-poolName']), nodeGroup = tolower(resourceGroup)\
         \n| join kind=leftouter (resources | where type =~ 'microsoft.containerservice/managedclusters' \
         | project cluster = name, nodeGroup = tolower(tostring(properties.nodeResourceGroup))) on nodeGroup\
         \n| project holder, nodes, per_node, pool, cluster\n| order by holder asc"
    );
    let mut found: Vec<Holder> = Vec::new();
    for row in rows(ctx, &azure.subscriptions, &nics, usize::MAX)?
        .iter()
        .chain(&rows(ctx, &azure.subscriptions, &sets, usize::MAX)?)
    {
        let Some(id) = text(&row["holder"]) else {
            continue;
        };
        let ips = row["ips"].as_u64().unwrap_or_else(|| {
            row["nodes"].as_u64().unwrap_or_default() * row["per_node"].as_u64().unwrap_or(1)
        });
        let pool = text(&row["cluster"])
            .zip(text(&row["pool"]))
            .map(|(cluster, pool)| format!("{cluster}/{pool}"));
        found.push(Holder {
            kind: target(&id).map(|held| held.kind()).unwrap_or_default(),
            id,
            ips,
            pool,
        });
    }
    found.sort_by(|a, b| b.ips.cmp(&a.ips).then_with(|| a.id.cmp(&b.id)));
    Ok(found)
}

fn subnet_get(ctx: &Ctx, args: SubnetGetArgs) -> Result<SubnetRow> {
    let azure = Azure::load(ctx)?;
    let id = subnet_id(ctx, &azure, &args)?;
    let subnet = read(ctx, &id, API, "net vnet list --fields id,subnets")?;
    let vnet = id[..id
        .to_ascii_lowercase()
        .rfind("/subnets/")
        .unwrap_or(id.len())]
        .to_owned();
    let usage = usages(ctx, &vnet)?
        .into_iter()
        .find(|(of, ..)| of.eq_ignore_ascii_case(&id));
    let holders = holders(ctx, &azure, &id)?;
    let held: u64 = holders.iter().map(|holder| holder.ips).sum();
    if let Some((_, used, _)) = usage
        && held < used
    {
        ctx.note(format!(
            "[the holders found hold {held} of the {used} IPs in use; app gateways, load balancers and gateways are not looked up]"
        ));
    }
    let props = &subnet["properties"];
    Ok(SubnetRow {
        name: text(&subnet["name"]).unwrap_or_default(),
        prefix: text(&props["addressPrefix"])
            .or_else(|| Some(strings(&props["addressPrefixes"]).join(", ")))
            .filter(|prefix| !prefix.is_empty()),
        used_ips: usage.as_ref().map(|(_, used, _)| *used),
        free_ips: usage.as_ref().map(|(_, _, free)| *free),
        nsg: text(&props["networkSecurityGroup"]["id"]),
        route_table: text(&props["routeTable"]["id"]),
        nat_gateway: text(&props["natGateway"]["id"]),
        service_endpoints: list(&props["serviceEndpoints"])
            .iter()
            .filter_map(|held| text(&held["service"]))
            .collect(),
        delegations: list(&props["delegations"])
            .iter()
            .filter_map(|held| text(&held["properties"]["serviceName"]))
            .collect(),
        private_endpoints: ids(&props["privateEndpoints"]),
        holders,
        id,
        vnet,
    })
}

command! {
    pub SUBNET_GET = ["net", "subnet", "get"], Read,
    "Show a subnet's used and free IPs, who holds them, its NSG and route table",
    keywords: ["full", "exhausted", "capacity", "usage", "free", "available", "addresses", "delegation"],
    example: "net subnet get vnet-contoso-prod/aks --group rg-contoso-prod",
    run: subnet_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RG, azure, inventory};

    #[test]
    fn a_subnet_by_vnet_and_name_shows_its_usage_and_who_holds_it() {
        let vnet = format!("{RG}/providers/Microsoft.Network/virtualNetworks/vnet-contoso");
        let aks = format!("{vnet}/subnets/aks");
        let vmss = "/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/MC_rg/providers/Microsoft.Compute/virtualMachineScaleSets/aks-user-1-vmss";
        let pe = format!("{RG}/providers/Microsoft.Network/privateEndpoints/pe-kv");
        let (outcome, transport) = azure(
            &[crate::NET],
            &[
                "net",
                "subnet",
                "get",
                "vnet-contoso/aks",
                "--group",
                "rg-contoso",
            ],
            vec![
                inventory(vec![json!({ "id": vnet })]),
                Answer::json(&json!({"id": aks, "name": "aks", "properties": {
                    "addressPrefix": "10.1.0.0/28",
                    "serviceEndpoints": [{"service": "Microsoft.Storage"}],
                    "privateEndpoints": [{"id": pe}]}})),
                Answer::json(&json!({"value": [{"id": aks, "currentValue": 10, "limit": 11}]})),
                inventory(vec![json!({"holder": pe, "ips": 1})]),
                inventory(vec![json!({"holder": vmss, "nodes": 3, "per_node": 2,
                    "pool": "user", "cluster": "aks-contoso"})]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            (got["used_ips"].clone(), got["free_ips"].clone()),
            (json!(10), json!(1))
        );
        assert_eq!(got["vnet"], vnet);
        assert_eq!(got["service_endpoints"], json!(["Microsoft.Storage"]));
        assert_eq!(
            got["holders"],
            json!([
                {"id": vmss, "type": "Microsoft.Compute/virtualMachineScaleSets", "ips": 6,
                    "pool": "aks-contoso/user"},
                {"id": pe, "type": "Microsoft.Network/privateEndpoints", "ips": 1},
            ])
        );
        assert!(
            outcome.stderr.contains("hold 7 of the 10 IPs in use"),
            "{}",
            outcome.stderr
        );
        let sent = transport.sent();
        assert_eq!(
            sent[1].url,
            format!("https://management.azure.com{aks}?api-version=2024-05-01")
        );
        let query = sent[3].body.clone().unwrap()["query"].clone();
        assert!(
            query.as_str().unwrap().contains(&format!(
                "where tostring(ip.properties.subnet.id) =~ '{aks}'"
            )),
            "{query}"
        );
    }

    #[test]
    fn a_bare_name_is_a_usage_error_naming_the_vnet_list() {
        let (outcome, transport) = azure(&[crate::NET], &["net", "subnet", "get", "aks"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("net vnet list"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().is_empty());
    }
}
