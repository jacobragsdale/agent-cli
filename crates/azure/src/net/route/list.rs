//! `net route list`: Resource Graph's route tables, one row per route, with
//! the subnets each table steers and whether gateway (BGP) routes reach
//! them: the forced-tunnel 0.0.0.0/0 to a firewall shows here.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{limited, text};
use crate::config::Azure;
use crate::net::{Place, ids, list};

const ROUTE_TABLE: &str = "Microsoft.Network/routeTables";

#[derive(clap::Args)]
pub struct RouteListArgs {
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RouteRow {
    /// The route's ARM id, which `arm resource get` takes; none for a table
    /// with no routes.
    id: Option<String>,
    /// The route table's ARM id.
    table: String,
    route: Option<String>,
    prefix: Option<String>,
    /// VirtualAppliance, VirtualNetworkGateway, VnetLocal, Internet or None (a drop).
    next_hop: Option<String>,
    next_hop_ip: Option<String>,
    /// False when the table turns off gateway-learned routes.
    bgp_propagation: bool,
    /// The ARM ids of the subnets it steers.
    subnets: Vec<String>,
}

fn route_list(ctx: &Ctx, args: RouteListArgs) -> Result<Vec<RouteRow>> {
    let azure = Azure::load(ctx)?;
    let project = "id, name, routes = properties.routes, subnets = properties.subnets, \
        disableBgp = tobool(properties.disableBgpRoutePropagation)";
    let mut found = Vec::new();
    for table in args
        .place
        .list(ctx, &azure, ROUTE_TABLE, project, args.limit)?
    {
        let row = |route: Option<&serde_json::Value>| {
            let props = route.map(|route| &route["properties"]);
            let field = |key: &str| props.and_then(|props| text(&props[key]));
            RouteRow {
                id: route.and_then(|route| text(&route["id"])),
                table: text(&table["id"]).unwrap_or_default(),
                route: route.and_then(|route| text(&route["name"])),
                prefix: field("addressPrefix"),
                next_hop: field("nextHopType"),
                next_hop_ip: field("nextHopIpAddress"),
                bgp_propagation: table["disableBgp"].as_bool() != Some(true),
                subnets: ids(&table["subnets"]),
            }
        };
        let routes = list(&table["routes"]);
        if routes.is_empty() {
            found.push(row(None));
        }
        found.extend(routes.iter().map(|route| row(Some(route))));
    }
    Ok(limited(ctx, found, args.limit))
}

command! {
    pub ROUTE_LIST = ["net", "route", "list"], Read,
    "List route tables and their routes, next hops, subnets and BGP propagation",
    keywords: ["udr", "table", "tables", "hop", "firewall", "appliance", "forced", "tunnel", "egress"],
    example: "net route list --fields table,prefix,next_hop,next_hop_ip,subnets",
    run: route_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{RG, azure, inventory};

    #[test]
    fn each_route_is_a_row_and_a_table_without_routes_still_shows() {
        let table = format!("{RG}/providers/Microsoft.Network/routeTables/rt-aks");
        let empty = format!("{RG}/providers/Microsoft.Network/routeTables/rt-empty");
        let (outcome, _) = azure(
            &[crate::NET],
            &["net", "route", "list"],
            vec![inventory(vec![
                json!({"id": empty, "name": "rt-empty", "routes": [], "subnets": null, "disableBgp": null}),
                json!({"id": table, "name": "rt-aks", "disableBgp": true,
                    "subnets": [{"id": "/x/subnets/aks"}],
                    "routes": [{"id": format!("{table}/routes/default"), "name": "default",
                        "properties": {"addressPrefix": "0.0.0.0/0",
                            "nextHopType": "VirtualAppliance", "nextHopIpAddress": "10.0.1.4"}}]}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"table": empty, "bgp_propagation": true},
                {"id": format!("{table}/routes/default"), "table": table, "route": "default",
                    "prefix": "0.0.0.0/0", "next_hop": "VirtualAppliance",
                    "next_hop_ip": "10.0.1.4", "bgp_propagation": false,
                    "subnets": ["/x/subnets/aks"]},
            ])
        );
    }
}
