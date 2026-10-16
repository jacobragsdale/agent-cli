//! `net zone list`: public and private DNS zones from Resource Graph, each
//! private zone joined to its vnet links in the same query, since a zone a
//! vnet is not linked to is invisible to every client in it.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::arm::quote;
use crate::client::{limited, text};
use crate::config::Azure;
use crate::net::{PRIVATE_ZONE, Place, VNET, ZONE, list};

#[derive(clap::Args)]
pub struct ZoneListArgs {
    /// Part of the zone's name: privatelink, vaultcore
    name: Option<String>,
    /// Only the private zones linked to this vnet: its ARM id, portal link, or name
    #[arg(long)]
    vnet: Option<String>,
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ZoneRow {
    /// The ARM id, which `net record list` takes.
    id: String,
    name: String,
    group: Option<String>,
    /// A private zone, which only linked vnets resolve.
    private: bool,
    /// Record sets, SOA and NS included.
    records: Option<u64>,
    /// A private zone's vnet links.
    links: Vec<Link>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Link {
    /// The vnet's ARM id.
    vnet: Option<String>,
    /// Whether the vnet's VMs register their own names in the zone.
    registration: Option<bool>,
    /// Completed, or InProgress while Azure sets it up.
    state: Option<String>,
}

fn zone_list(ctx: &Ctx, args: ZoneListArgs) -> Result<Vec<ZoneRow>> {
    let azure = Azure::load(ctx)?;
    let vnet = match &args.vnet {
        Some(raw) => Some(
            Place::default()
                .find(ctx, &azure, raw, &[VNET], "net vnet list", "net vnet get")?
                .id,
        ),
        None => None,
    };
    let mut tail = String::new();
    if let Some(name) = &args.name {
        tail.push_str(&format!("\n| where name contains {}", quote(name)));
    }
    // VERIFY(work): that Resource Graph holds private zones' vnet links as
    // resources of their own.
    tail.push_str(
        "\n| extend zone = tolower(id)\
         \n| join kind=leftouter (resources | where type =~ 'microsoft.network/privatednszones/virtualnetworklinks' \
         | extend zone = tolower(substring(id, 0, indexof(tolower(id), '/virtualnetworklinks/'))) \
         | summarize links = make_list(pack('vnet', tostring(properties.virtualNetwork.id), \
         'registration', tobool(properties.registrationEnabled), \
         'state', tostring(properties.virtualNetworkLinkState))) by zone) on zone\
         \n| project id, name, type, resourceGroup, records = tolong(properties.numberOfRecordSets), links\
         \n| order by name asc, id asc",
    );
    // All of them, so --vnet keeps to the limit after it filters.
    let found = args
        .place
        .rows(ctx, &azure, &[ZONE, PRIVATE_ZONE], &tail, usize::MAX)?;
    let rows: Vec<ZoneRow> = found
        .iter()
        .map(|row| ZoneRow {
            id: text(&row["id"]).unwrap_or_default(),
            name: text(&row["name"]).unwrap_or_default(),
            group: text(&row["resourceGroup"]),
            private: text(&row["type"]).is_some_and(|kind| kind.eq_ignore_ascii_case(PRIVATE_ZONE)),
            records: row["records"].as_u64(),
            links: list(&row["links"])
                .iter()
                .map(|link| Link {
                    vnet: text(&link["vnet"]),
                    registration: link["registration"].as_bool(),
                    state: text(&link["state"]),
                })
                .collect(),
        })
        .filter(|zone| {
            vnet.as_deref().is_none_or(|vnet| {
                zone.links.iter().any(|link| {
                    link.vnet
                        .as_deref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(vnet))
                })
            })
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub ZONE_LIST = ["net", "zone", "list"], Read,
    "List public and private DNS zones and the vnets each private zone is linked to",
    keywords: ["zones", "private", "privatelink", "linked", "link", "resolve", "domain"],
    example: "net zone list privatelink --fields name,links",
    run: zone_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{RG, azure, inventory};

    #[test]
    fn zones_carry_their_vnet_links_and_vnet_keeps_the_linked_ones() {
        let vnet = format!("{RG}/providers/Microsoft.Network/virtualNetworks/vnet-aks");
        let zone = |name: &str, kind: &str, links: serde_json::Value| {
            json!({"id": format!("{RG}/providers/{kind}/{name}"), "name": name,
                "type": kind.to_ascii_lowercase(), "resourceGroup": "rg-contoso", "records": 3,
                "links": links})
        };
        let answer = || {
            inventory(vec![
                zone("contoso.example", "Microsoft.Network/dnszones", json!(null)),
                zone(
                    "privatelink.blob.core.windows.net",
                    "Microsoft.Network/privateDnsZones",
                    json!([{"vnet": vnet.to_uppercase(), "registration": false, "state": "Completed"}]),
                ),
                zone(
                    "privatelink.vaultcore.azure.net",
                    "Microsoft.Network/privateDnsZones",
                    json!([{"vnet": "/x/virtualNetworks/vnet-hub", "registration": false, "state": "Completed"}]),
                ),
            ])
        };
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "zone", "list", "--limit", "2"],
            vec![answer()],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got[0],
            json!({"id": format!("{RG}/providers/Microsoft.Network/dnszones/contoso.example"),
            "name": "contoso.example", "group": "rg-contoso", "private": false, "records": 3})
        );
        assert_eq!(got[1]["links"][0]["state"], "Completed");
        assert!(
            outcome.stderr.contains("[2 of 3; --limit N]"),
            "{}",
            outcome.stderr
        );
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert!(
            query.as_str().unwrap().starts_with(
                "resources\n| where type in~ ('Microsoft.Network/dnsZones', 'Microsoft.Network/privateDnsZones')\n| extend zone"
            ),
            "{query}"
        );

        let (outcome, _) = azure(
            &[crate::NET],
            &["net", "zone", "list", "--vnet", &vnet],
            vec![answer()],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got.as_array().map(Vec::len), Some(1), "{got}");
        assert_eq!(got[0]["name"], "privatelink.blob.core.windows.net");
    }
}
