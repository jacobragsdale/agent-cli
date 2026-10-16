//! `net endpoint get`: a private endpoint, its NIC's IPs and the names they
//! answer for, and for each record its DNS zone group registered: whether
//! the zone still holds it at the endpoint's IP, and whether the zone is
//! linked to the endpoint's vnet. A missing link is the usual reason a
//! client in that vnet resolves the service's public address and times out.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::text;
use crate::config::Azure;
use crate::net::endpoint::connection;
use crate::net::{API, ENDPOINT, PRIVATE_DNS_API, Place, found, ids, list, read, strings};

const LIST: &str = "net endpoint list";

#[derive(clap::Args)]
pub struct EndpointGetArgs {
    /// The private endpoint: its ARM id, portal link, or name
    endpoint: String,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct EndpointRow {
    /// The ARM id, which `net endpoint get` takes.
    id: String,
    name: String,
    /// The ARM id of what it reaches privately.
    target: Option<String>,
    group_ids: Vec<String>,
    /// Approved, Pending, Rejected or Disconnected.
    status: Option<String>,
    /// The subnet's ARM id, which `net subnet get` takes.
    subnet: Option<String>,
    /// The vnet's ARM id: where a client must be, or be linked from, to resolve it.
    vnet: Option<String>,
    ips: Vec<String>,
    /// The service's names that should resolve to `ips`.
    fqdns: Vec<String>,
    /// Each record its DNS zone group keeps; empty when it has none.
    dns: Vec<Dns>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Dns {
    /// The record's name in the private zone.
    fqdn: Option<String>,
    /// The private DNS zone's ARM id, which `net record list` takes.
    zone: String,
    /// The A record's addresses as the zone holds it now; empty when it holds none.
    record: Vec<String>,
    /// Whether the record holds one of the endpoint's IPs.
    resolves: bool,
    /// Whether the zone is linked to the endpoint's vnet.
    linked: bool,
}

fn endpoint_get(ctx: &Ctx, args: EndpointGetArgs) -> Result<EndpointRow> {
    let azure = Azure::load(ctx)?;
    let found_at = args.place.find(
        ctx,
        &azure,
        &args.endpoint,
        &[ENDPOINT],
        LIST,
        "net endpoint get",
    )?;
    let endpoint = read(ctx, &found_at.id, API, LIST)?;
    let props = &endpoint["properties"];
    let (target, group_ids, status) = connection(
        &props["privateLinkServiceConnections"],
        &props["manualPrivateLinkServiceConnections"],
    );
    let subnet = text(&props["subnet"]["id"]);
    let vnet = subnet.as_deref().and_then(|subnet| {
        let at = subnet.to_ascii_lowercase().rfind("/subnets/")?;
        Some(subnet[..at].to_owned())
    });
    let (mut ips, mut fqdns) = (Vec::new(), Vec::new());
    for nic in ids(&props["networkInterfaces"]) {
        let nic = read(ctx, &nic, API, LIST)?;
        for config in list(&nic["properties"]["ipConfigurations"]) {
            let config = &config["properties"];
            ips.extend(text(&config["privateIPAddress"]));
            fqdns.extend(strings(&config["privateLinkConnectionProperties"]["fqdns"]));
        }
    }
    let groups = read(
        ctx,
        &format!("{}/privateDnsZoneGroups", found_at.id),
        API,
        LIST,
    )?;
    let mut dns = Vec::new();
    for config in list(&groups["value"])
        .iter()
        .flat_map(|group| list(&group["properties"]["privateDnsZoneConfigs"]))
    {
        let Some(zone) = text(&config["properties"]["privateDnsZoneId"]) else {
            continue;
        };
        let links = read(
            ctx,
            &format!("{zone}/virtualNetworkLinks"),
            PRIVATE_DNS_API,
            "net zone list",
        )?;
        // ponytail: a vnet whose DNS servers forward to a resolver in a
        // linked vnet resolves the zone without a link of its own; that
        // still reads as unlinked here.
        let linked = vnet.as_deref().is_some_and(|vnet| {
            list(&links["value"]).iter().any(|link| {
                text(&link["properties"]["virtualNetwork"]["id"])
                    .is_some_and(|linked| linked.eq_ignore_ascii_case(vnet))
            })
        });
        for set in list(&config["properties"]["recordSets"])
            .iter()
            .filter(|set| text(&set["recordType"]).is_none_or(|kind| kind == "A"))
        {
            let name = text(&set["recordSetName"]).unwrap_or_default();
            let record: Vec<String> = found(ctx, &format!("{zone}/A/{name}"), PRIVATE_DNS_API)?
                .map(|held| {
                    list(&held["properties"]["aRecords"])
                        .iter()
                        .filter_map(|a| text(&a["ipv4Address"]))
                        .collect()
                })
                .unwrap_or_default();
            dns.push(Dns {
                fqdn: text(&set["fqdn"]),
                zone: zone.clone(),
                resolves: record.iter().any(|ip| ips.contains(ip)),
                record,
                linked,
            });
        }
    }
    let row = EndpointRow {
        id: found_at.id,
        name: text(&endpoint["name"]).unwrap_or_default(),
        target,
        group_ids,
        status,
        subnet,
        vnet,
        ips,
        fqdns,
        dns,
    };
    if let Some(next) = next(&row) {
        ctx.note(next);
    }
    Ok(row)
}

/// The one next step when the service's name would not resolve to the
/// endpoint from its own vnet: no zone group, a zone not linked there, or a
/// record that is gone or points elsewhere.
fn next(row: &EndpointRow) -> Option<String> {
    let last = |id: &str| id.rsplit('/').next().unwrap_or(id).to_owned();
    let service = row.fqdns.first().unwrap_or(&row.name);
    if row.dns.is_empty() {
        return Some(format!(
            "[next: agent-cli net zone list privatelink  (the endpoint has no DNS zone group, so {service} resolves to its public address unless the clients' DNS server answers it)]"
        ));
    }
    if let Some(dns) = row.dns.iter().find(|dns| !dns.linked) {
        return Some(format!(
            "[next: agent-cli net zone list {}  (the zone is not linked to vnet {}, so clients there resolve {service} to its public address)]",
            last(&dns.zone),
            row.vnet.as_deref().map_or_else(|| "?".to_owned(), last),
        ));
    }
    let dns = row.dns.iter().find(|dns| !dns.resolves)?;
    Some(format!(
        "[next: agent-cli net record list {}  (the zone holds no A record for {} at {})]",
        dns.zone,
        dns.fqdn.as_deref().unwrap_or(service),
        row.ips.join(", "),
    ))
}

command! {
    pub ENDPOINT_GET = ["net", "endpoint", "get"], Read,
    "Show a private endpoint's IPs and whether its name resolves to it from its vnet",
    keywords: ["private", "privatelink", "resolve", "resolving", "linked", "fqdn"],
    example: "net endpoint get pe-kv-contoso-prod --fields ips,dns",
    run: endpoint_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RG, azure};

    const ZONE: &str = "/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-contoso/providers/Microsoft.Network/privateDnsZones/privatelink.vaultcore.azure.net";

    fn endpoint(id: &str, vnet: &str) -> Answer {
        Answer::json(&json!({"id": id, "name": "pe-kv", "properties": {
            "subnet": {"id": format!("{vnet}/subnets/pe")},
            "networkInterfaces": [{"id": "/nic/pe-kv.nic"}],
            "privateLinkServiceConnections": [{"properties": {"privateLinkServiceId": "/vault/kv",
                "groupIds": ["vault"], "privateLinkServiceConnectionState": {"status": "Approved"}}}]}}))
    }

    fn nic() -> Answer {
        Answer::json(&json!({"properties": {"ipConfigurations": [{"properties": {
            "privateIPAddress": "10.0.4.4",
            "privateLinkConnectionProperties": {"groupId": "vault", "fqdns": ["kv.vault.azure.net"]}}}]}}))
    }

    fn groups() -> Answer {
        Answer::json(
            &json!({"value": [{"name": "default", "properties": {"privateDnsZoneConfigs": [
            {"name": "vault", "properties": {"privateDnsZoneId": ZONE, "recordSets": [
                {"recordType": "A", "recordSetName": "kv",
                 "fqdn": "kv.privatelink.vaultcore.azure.net", "ipAddresses": ["10.0.4.4"]}]}}]}}]}),
        )
    }

    fn links(vnet: &str) -> Answer {
        Answer::json(&json!({"value": [{"name": "hub", "properties": {
            "virtualNetwork": {"id": vnet}, "registrationEnabled": false,
            "virtualNetworkLinkState": "Completed"}}]}))
    }

    #[test]
    fn a_zone_not_linked_to_the_endpoints_vnet_names_zone_list_next() {
        let id = format!("{RG}/providers/Microsoft.Network/privateEndpoints/pe-kv");
        let vnet = format!("{RG}/providers/Microsoft.Network/virtualNetworks/vnet-aks");
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "endpoint", "get", &id],
            vec![
                endpoint(&id, &vnet),
                nic(),
                groups(),
                links(
                    "/subscriptions/s/resourceGroups/hub/providers/Microsoft.Network/virtualNetworks/vnet-hub",
                ),
                Answer::json(
                    &json!({"properties": {"ttl": 10, "aRecords": [{"ipv4Address": "10.0.4.4"}]}}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["vnet"], vnet);
        assert_eq!(got["ips"], json!(["10.0.4.4"]));
        assert_eq!(got["fqdns"], json!(["kv.vault.azure.net"]));
        assert_eq!(
            got["dns"],
            json!([{"fqdn": "kv.privatelink.vaultcore.azure.net", "zone": ZONE,
                "record": ["10.0.4.4"], "resolves": true, "linked": false}])
        );
        assert!(
            outcome.stderr.contains(
                "[next: agent-cli net zone list privatelink.vaultcore.azure.net  (the zone is not linked to vnet vnet-aks"
            ),
            "{}",
            outcome.stderr
        );
        let urls: Vec<String> = transport
            .sent()
            .iter()
            .map(|sent| sent.url.clone())
            .collect();
        assert_eq!(
            urls[2],
            format!("https://management.azure.com{id}/privateDnsZoneGroups?api-version=2024-05-01")
        );
        assert_eq!(
            urls[4],
            format!("https://management.azure.com{ZONE}/A/kv?api-version=2020-06-01")
        );
    }

    #[test]
    fn a_record_gone_from_a_linked_zone_names_record_list_next() {
        let id = format!("{RG}/providers/Microsoft.Network/privateEndpoints/pe-kv");
        let vnet = format!("{RG}/providers/Microsoft.Network/virtualNetworks/vnet-aks");
        let (outcome, _) = azure(
            &[crate::NET],
            &["net", "endpoint", "get", &id],
            vec![
                endpoint(&id, &vnet),
                nic(),
                groups(),
                links(&vnet.to_uppercase()),
                Answer::status(404, r#"{"error":{"code":"NotFound"}}"#),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            (
                got["dns"][0]["linked"].clone(),
                got["dns"][0]["resolves"].clone()
            ),
            (json!(true), json!(false))
        );
        assert!(
            outcome.stderr.contains(&format!(
                "[next: agent-cli net record list {ZONE}  (the zone holds no A record for kv.privatelink.vaultcore.azure.net at 10.0.4.4)]"
            )),
            "{}",
            outcome.stderr
        );
    }
}
