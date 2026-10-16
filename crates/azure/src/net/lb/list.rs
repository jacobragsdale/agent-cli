//! `net lb list`: Resource Graph's load balancers with how many frontends,
//! rules and backend pools each has, AKS's `kubernetes` among them.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::text;
use crate::config::Azure;
use crate::net::{LB, Place};

#[derive(clap::Args)]
pub struct LbListArgs {
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LbListRow {
    /// The ARM id, which `net lb get` takes.
    id: String,
    name: String,
    group: Option<String>,
    location: Option<String>,
    /// Basic or Standard.
    sku: Option<String>,
    frontends: u64,
    rules: u64,
    pools: u64,
}

fn lb_list(ctx: &Ctx, args: LbListArgs) -> Result<Vec<LbListRow>> {
    let azure = Azure::load(ctx)?;
    let project = "id, name, resourceGroup, location, sku = tostring(sku.name), \
        frontends = array_length(properties.frontendIPConfigurations), \
        rules = array_length(properties.loadBalancingRules), \
        pools = array_length(properties.backendAddressPools)";
    Ok(args
        .place
        .list(ctx, &azure, LB, project, args.limit)?
        .iter()
        .map(|row| LbListRow {
            id: text(&row["id"]).unwrap_or_default(),
            name: text(&row["name"]).unwrap_or_default(),
            group: text(&row["resourceGroup"]),
            location: text(&row["location"]),
            sku: text(&row["sku"]),
            frontends: row["frontends"].as_u64().unwrap_or_default(),
            rules: row["rules"].as_u64().unwrap_or_default(),
            pools: row["pools"].as_u64().unwrap_or_default(),
        })
        .collect())
}

command! {
    pub LB_LIST = ["net", "lb", "list"], Read,
    "List load balancers with their counts of frontends, rules and backend pools",
    keywords: ["load", "balancer", "balancers", "frontend", "backend"],
    example: "net lb list --fields id,frontends,rules",
    run: lb_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{RG, azure, inventory};

    #[test]
    fn a_load_balancer_row_counts_its_parts() {
        let id = format!("{RG}/providers/Microsoft.Network/loadBalancers/kubernetes");
        let (outcome, _) = azure(
            &[crate::NET],
            &["net", "lb", "list"],
            vec![inventory(vec![json!({"id": id, "name": "kubernetes",
                "resourceGroup": "rg-contoso", "location": "eastus", "sku": "Standard",
                "frontends": 2, "rules": 2, "pools": 2})])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": id, "name": "kubernetes", "group": "rg-contoso", "location": "eastus",
                "sku": "Standard", "frontends": 2, "rules": 2, "pools": 2}])
        );
    }
}
