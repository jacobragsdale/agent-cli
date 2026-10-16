//! `net nsg list`: Resource Graph's network security groups with what each
//! is attached to, so an NSG attached to nothing (and so guarding nothing)
//! shows as such.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::text;
use crate::config::Azure;
use crate::net::{NSG, Place, ids};

#[derive(clap::Args)]
pub struct NsgListArgs {
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NsgListRow {
    /// The ARM id, which `net nsg get` takes.
    id: String,
    name: String,
    group: Option<String>,
    location: Option<String>,
    /// Custom rules; every NSG also has Azure's six defaults.
    rules: u64,
    /// The subnets' ARM ids.
    subnets: Vec<String>,
    /// The NICs' ARM ids.
    nics: Vec<String>,
}

fn nsg_list(ctx: &Ctx, args: NsgListArgs) -> Result<Vec<NsgListRow>> {
    let azure = Azure::load(ctx)?;
    let project = "id, name, resourceGroup, location, rules = array_length(properties.securityRules), \
        subnets = properties.subnets, nics = properties.networkInterfaces";
    Ok(args
        .place
        .list(ctx, &azure, NSG, project, args.limit)?
        .iter()
        .map(|row| NsgListRow {
            id: text(&row["id"]).unwrap_or_default(),
            name: text(&row["name"]).unwrap_or_default(),
            group: text(&row["resourceGroup"]),
            location: text(&row["location"]),
            rules: row["rules"].as_u64().unwrap_or_default(),
            subnets: ids(&row["subnets"]),
            nics: ids(&row["nics"]),
        })
        .collect())
}

command! {
    pub NSG_LIST = ["net", "nsg", "list"], Read,
    "List network security groups with their rule counts and what they guard",
    keywords: ["security", "groups", "attached", "associated"],
    example: "net nsg list --fields id,rules,subnets",
    run: nsg_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{RG, azure, inventory};

    #[test]
    fn an_nsg_row_names_the_subnets_and_nics_it_guards() {
        let nsg = format!("{RG}/providers/Microsoft.Network/networkSecurityGroups/nsg-aks");
        let subnet = format!("{RG}/providers/Microsoft.Network/virtualNetworks/v/subnets/aks");
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "nsg", "list"],
            vec![inventory(vec![
                json!({"id": nsg, "name": "nsg-aks", "resourceGroup": "rg-contoso",
                "location": "westeurope", "rules": 2, "subnets": [{"id": subnet}], "nics": null}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": nsg, "name": "nsg-aks", "group": "rg-contoso",
                "location": "westeurope", "rules": 2, "subnets": [subnet]}])
        );
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert!(
            query
                .as_str()
                .unwrap()
                .starts_with("resources\n| where type =~ 'Microsoft.Network/networkSecurityGroups'\n| project id,"),
            "{query}"
        );
    }
}
