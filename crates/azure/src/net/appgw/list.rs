//! `net appgw list`: Resource Graph's application gateways with their SKU,
//! state, WAF mode and how many listeners and backend pools each has.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::text;
use crate::config::Azure;
use crate::net::{APPGW, Place};

#[derive(clap::Args)]
pub struct AppgwListArgs {
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AppgwListRow {
    /// The ARM id, which `net appgw get` takes.
    id: String,
    name: String,
    group: Option<String>,
    location: Option<String>,
    /// Standard_v2 or WAF_v2.
    sku: Option<String>,
    /// Running or Stopped.
    status: Option<String>,
    /// Detection or Prevention, when the WAF is set on the gateway itself.
    waf_mode: Option<String>,
    /// The WAF policy's ARM id, when one is attached.
    waf_policy: Option<String>,
    listeners: u64,
    pools: u64,
}

fn appgw_list(ctx: &Ctx, args: AppgwListArgs) -> Result<Vec<AppgwListRow>> {
    let azure = Azure::load(ctx)?;
    let project = "id, name, resourceGroup, location, sku = tostring(properties.sku.name), \
        status = tostring(properties.operationalState), \
        wafMode = tostring(properties.webApplicationFirewallConfiguration.firewallMode), \
        wafPolicy = tostring(properties.firewallPolicy.id), \
        listeners = array_length(properties.httpListeners), \
        pools = array_length(properties.backendAddressPools)";
    Ok(args
        .place
        .list(ctx, &azure, APPGW, project, args.limit)?
        .iter()
        .map(|row| AppgwListRow {
            id: text(&row["id"]).unwrap_or_default(),
            name: text(&row["name"]).unwrap_or_default(),
            group: text(&row["resourceGroup"]),
            location: text(&row["location"]),
            sku: text(&row["sku"]),
            status: text(&row["status"]),
            waf_mode: text(&row["wafMode"]),
            waf_policy: text(&row["wafPolicy"]),
            listeners: row["listeners"].as_u64().unwrap_or_default(),
            pools: row["pools"].as_u64().unwrap_or_default(),
        })
        .collect())
}

command! {
    pub APPGW_LIST = ["net", "appgw", "list"], Read,
    "List application gateways with their SKU, status and WAF mode",
    keywords: ["application", "gateway", "gateways", "waf", "firewall", "ingress", "agic"],
    example: "net appgw list --fields id,sku,status,waf_mode",
    run: appgw_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{RG, azure, inventory};

    #[test]
    fn an_appgw_row_has_its_waf_mode_and_counts() {
        let id = format!("{RG}/providers/Microsoft.Network/applicationGateways/agw");
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "appgw", "list"],
            vec![inventory(vec![json!({"id": id, "name": "agw",
                "resourceGroup": "rg-contoso", "location": "eastus", "sku": "WAF_v2",
                "status": "Running", "wafMode": "Prevention", "wafPolicy": "",
                "listeners": 1, "pools": 1})])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": id, "name": "agw", "group": "rg-contoso", "location": "eastus",
                "sku": "WAF_v2", "status": "Running", "waf_mode": "Prevention",
                "listeners": 1, "pools": 1}])
        );
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert!(
            query.as_str().unwrap().starts_with(
                "resources\n| where type =~ 'Microsoft.Network/applicationGateways'\n| project"
            ),
            "{query}"
        );
    }
}
