//! What `ip list` and `lb get` share: public IPs from Resource Graph, each
//! with what holds it and the k8s service AKS made it for.

pub(crate) mod list;

use agent_cli_core::Ctx;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::text;
use crate::config::Azure;
use crate::net::{PUBLIC_IP, Place};

#[derive(Debug, Serialize, JsonSchema)]
pub struct PublicIp {
    /// The ARM id, which `arm resource get` takes.
    pub id: String,
    pub name: String,
    pub group: Option<String>,
    pub address: Option<String>,
    /// Basic or Standard.
    pub sku: Option<String>,
    /// Static or Dynamic.
    pub allocation: Option<String>,
    /// The ARM id of what uses it: a load balancer frontend, an application
    /// gateway frontend, a NIC's IP configuration or a NAT gateway; none when
    /// it is spare.
    pub held_by: Option<String>,
    /// The k8s service it was made for, `cluster/namespace/name` as `k8s
    /// service get` takes it, or `namespace/name` outside an AKS node group.
    pub service: Option<String>,
}

/// The public IPs within `place` that `filter` (KQL after `| where`) keeps,
/// sorted by name; the AKS cluster is the one whose node group holds them.
pub(crate) fn public_ips(
    ctx: &Ctx,
    azure: &Azure,
    place: &Place,
    filter: Option<&str>,
    limit: usize,
) -> Result<Vec<PublicIp>> {
    let mut tail = filter.map_or_else(String::new, |filter| format!("\n| where {filter}"));
    // VERIFY(work): that AKS tags the public IPs it makes for a LoadBalancer
    // service `k8s-azure-service: namespace/name`.
    tail.push_str(
        "\n| extend nodeGroup = tolower(resourceGroup)\
         \n| join kind=leftouter (resources | where type =~ 'microsoft.containerservice/managedclusters' \
         | project cluster = name, nodeGroup = tolower(tostring(properties.nodeResourceGroup))) on nodeGroup\
         \n| project id, name, resourceGroup, address = tostring(properties.ipAddress), sku = tostring(sku.name), \
         allocation = tostring(properties.publicIPAllocationMethod), \
         heldBy = coalesce(tostring(properties.ipConfiguration.id), tostring(properties.natGateway.id)), \
         service = tostring(tags['k8s-azure-service']), cluster\
         \n| order by name asc, id asc",
    );
    Ok(place
        .rows(ctx, azure, &[PUBLIC_IP], &tail, limit)?
        .iter()
        .map(public_ip)
        .collect())
}

fn public_ip(row: &Value) -> PublicIp {
    let service = text(&row["service"]).map(|service| match text(&row["cluster"]) {
        Some(cluster) => format!("{cluster}/{service}"),
        None => service,
    });
    PublicIp {
        id: text(&row["id"]).unwrap_or_default(),
        name: text(&row["name"]).unwrap_or_default(),
        group: text(&row["resourceGroup"]),
        address: text(&row["address"]),
        sku: text(&row["sku"]),
        allocation: text(&row["allocation"]),
        held_by: text(&row["heldBy"]),
        service,
    }
}
