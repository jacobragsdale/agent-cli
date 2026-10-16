//! `net nsg get`: an NSG's custom and default rules in one list, in the
//! order Azure applies them (direction, then priority), since a deny that
//! shadows an allow is only visible beside it.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::text;
use crate::config::Azure;
use crate::net::{API, NSG, Place, Rule, ids, nsg_rules, read};

#[derive(clap::Args)]
pub struct NsgGetArgs {
    /// The NSG: its ARM id, portal link, or name
    nsg: String,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NsgRow {
    /// The ARM id, which `net nsg get` takes.
    id: String,
    name: String,
    location: Option<String>,
    /// The subnets' ARM ids.
    subnets: Vec<String>,
    /// The NICs' ARM ids.
    nics: Vec<String>,
    /// Inbound then outbound, each by priority: the first match decides.
    rules: Vec<Rule>,
}

fn nsg_get(ctx: &Ctx, args: NsgGetArgs) -> Result<NsgRow> {
    let azure = Azure::load(ctx)?;
    let target = args.place.find(
        ctx,
        &azure,
        &args.nsg,
        &[NSG],
        "net nsg list",
        "net nsg get",
    )?;
    let nsg = read(ctx, &target.id, API, "net nsg list")?;
    let mut rules = nsg_rules(&nsg, false);
    rules.sort_by_key(|rule| {
        (
            !rule.direction.eq_ignore_ascii_case("Inbound"),
            rule.priority,
        )
    });
    let props = &nsg["properties"];
    Ok(NsgRow {
        id: text(&nsg["id"]).unwrap_or(target.id),
        name: text(&nsg["name"]).unwrap_or_default(),
        location: text(&nsg["location"]),
        subnets: ids(&props["subnets"]),
        nics: ids(&props["networkInterfaces"]),
        rules,
    })
}

command! {
    pub NSG_GET = ["net", "nsg", "get"], Read,
    "Show an NSG's inbound and outbound rules by priority, defaults included",
    keywords: ["security", "group", "port", "blocked", "deny", "allow", "inbound", "outbound"],
    example: "net nsg get nsg-contoso-aks --fields rules",
    run: nsg_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RG, azure};

    #[test]
    fn rules_come_inbound_first_by_priority_with_the_defaults() {
        let nsg = format!("{RG}/providers/Microsoft.Network/networkSecurityGroups/nsg-aks");
        let rule = |name: &str, direction: &str, priority: u64, access: &str, port: &str| {
            json!({"name": name, "properties": {"direction": direction, "priority": priority,
                "access": access, "protocol": "Tcp", "sourceAddressPrefix": "Internet",
                "sourcePortRange": "*", "destinationAddressPrefix": "*", "destinationPortRange": port}})
        };
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "nsg", "get", &nsg],
            vec![Answer::json(
                &json!({"id": nsg, "name": "nsg-aks", "properties": {
                "securityRules": [
                    rule("deny-smtp-out", "Outbound", 200, "Deny", "25"),
                    rule("deny-ssh-in", "Inbound", 200, "Deny", "22"),
                    rule("allow-https-in", "Inbound", 100, "Allow", "443"),
                ],
                "defaultSecurityRules": [rule("DenyAllInBound", "Inbound", 65500, "Deny", "*")],
                "subnets": [{"id": "/x/subnets/aks"}]}}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        let order: Vec<(&str, bool)> = got["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| {
                (
                    rule["name"].as_str().unwrap(),
                    rule["default"].as_bool().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            order,
            [
                ("allow-https-in", false),
                ("deny-ssh-in", false),
                ("DenyAllInBound", true),
                ("deny-smtp-out", false),
            ]
        );
        assert_eq!(got["rules"][1]["destination_ports"], json!(["22"]));
        assert_eq!(got["subnets"], json!(["/x/subnets/aks"]));
        assert_eq!(
            transport.sent()[0].url,
            format!("https://management.azure.com{nsg}?api-version=2024-05-01")
        );
    }
}
