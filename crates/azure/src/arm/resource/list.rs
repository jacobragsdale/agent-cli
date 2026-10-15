//! `arm resource list`: Resource Graph's `resources`, any type, filtered in
//! the query so `--limit` bounds what comes back, not what is read.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::arm::{quote, scope};
use crate::client::text;
use crate::config::Azure;
use crate::graph::rows;

#[derive(clap::Args)]
pub struct ResourceListArgs {
    /// Part of the resource name, any case
    name: Option<String>,
    /// Only this type, any case: Microsoft.Network/virtualNetworks
    #[arg(long = "type")]
    kind: Option<String>,
    /// Only this resource group
    #[arg(long)]
    group: Option<String>,
    /// Only resources tagged key, or key=value (value any case; repeatable)
    #[arg(long)]
    tag: Vec<String>,
    /// Only this region: westeurope
    #[arg(long)]
    location: Option<String>,
    /// Only this subscription: its GUID or name (within [azure] subscriptions)
    #[arg(long)]
    subscription: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ResourceRow {
    /// The ARM id, which `arm resource get` takes.
    id: String,
    name: String,
    /// As Resource Graph writes it, lower case.
    #[serde(rename = "type")]
    kind: String,
    group: Option<String>,
    location: Option<String>,
    sku: Option<String>,
    provisioning_state: Option<String>,
}

fn kql(args: &ResourceListArgs) -> String {
    let mut kql = String::from("resources");
    if let Some(name) = &args.name {
        kql.push_str(&format!("\n| where name contains {}", quote(name)));
    }
    for (column, value) in [
        ("type", &args.kind),
        ("resourceGroup", &args.group),
        ("location", &args.location),
    ] {
        if let Some(value) = value {
            kql.push_str(&format!("\n| where {column} =~ {}", quote(value)));
        }
    }
    for tag in &args.tag {
        kql.push_str(&match tag.split_once('=') {
            Some((key, value)) => format!(
                "\n| where tostring(tags[{}]) =~ {}",
                quote(key),
                quote(value)
            ),
            None => format!("\n| where isnotempty(tags[{}])", quote(tag)),
        });
    }
    kql.push_str(
        "\n| project id, name, type, resourceGroup, location, sku = tostring(sku.name), \
         provisioningState = tostring(properties.provisioningState)\n| order by name asc, id asc",
    );
    kql
}

fn resource_list(ctx: &Ctx, args: ResourceListArgs) -> Result<Vec<ResourceRow>> {
    let azure = Azure::load(ctx)?;
    let scope = scope(ctx, &azure, args.subscription.as_deref())?;
    Ok(rows(ctx, &scope, &kql(&args), args.limit)?
        .into_iter()
        .map(|row| ResourceRow {
            id: text(&row["id"]).unwrap_or_default(),
            name: text(&row["name"]).unwrap_or_default(),
            kind: text(&row["type"]).unwrap_or_default(),
            group: text(&row["resourceGroup"]),
            location: text(&row["location"]),
            sku: text(&row["sku"]),
            provisioning_state: text(&row["provisioningState"]),
        })
        .collect())
}

command! {
    pub RESOURCE_LIST = ["arm", "resource", "list"], Read,
    "List Azure resources of any type by name, type, group, tag or region",
    keywords: ["find", "inventory", "resources", "graph", "provisioning", "sku", "region", "tagged", "all"],
    example: "arm resource list --type Microsoft.Network/virtualNetworks --fields id,location",
    run: resource_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{azure_with, inventory};

    const SUB: &str = "00000000-0000-4000-8000-000000000001";

    #[test]
    fn every_filter_goes_into_the_query_quoted_and_the_subscription_is_resolved_by_name() {
        let vnet = json!({
            "id": format!("/subscriptions/{SUB}/resourceGroups/rg-contoso-prod/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod"),
            "name": "vnet-contoso-prod", "type": "microsoft.network/virtualnetworks",
            "resourceGroup": "rg-contoso-prod", "location": "westeurope", "sku": "",
            "provisioningState": "Succeeded",
        });
        let (outcome, transport) = azure_with(
            &[crate::ARM],
            &[
                "arm",
                "resource",
                "list",
                "vnet",
                "--type",
                "Microsoft.Network/virtualNetworks",
                "--group",
                "rg-contoso-prod",
                "--tag",
                "env=prod",
                "--tag",
                "owner",
                "--location",
                "westeurope",
                "--subscription",
                "contoso-prod",
            ],
            vec![
                Answer::json(
                    &json!({"value": [{"subscriptionId": SUB, "displayName": "contoso-prod",
                    "state": "Enabled", "tenantId": "t"}]}),
                ),
                inventory(vec![vnet]),
            ],
            "[azure]\n",
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": format!("/subscriptions/{SUB}/resourceGroups/rg-contoso-prod/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod"),
                "name": "vnet-contoso-prod", "type": "microsoft.network/virtualnetworks",
                "group": "rg-contoso-prod", "location": "westeurope", "provisioning_state": "Succeeded"}])
        );
        let body = transport.sent()[1].body.clone().unwrap();
        assert_eq!(body["subscriptions"], json!([SUB]));
        assert_eq!(
            body["query"],
            "resources\n| where name contains 'vnet'\n| where type =~ 'Microsoft.Network/virtualNetworks'\
             \n| where resourceGroup =~ 'rg-contoso-prod'\n| where location =~ 'westeurope'\
             \n| where tostring(tags['env']) =~ 'prod'\n| where isnotempty(tags['owner'])\
             \n| project id, name, type, resourceGroup, location, sku = tostring(sku.name), \
             provisioningState = tostring(properties.provisioningState)\n| order by name asc, id asc"
        );
    }

    #[test]
    fn a_subscription_the_allowlist_leaves_out_is_a_usage_error() {
        let (outcome, transport) = azure_with(
            &[crate::ARM],
            &["arm", "resource", "list", "--subscription", "contoso-dev"],
            vec![Answer::json(&json!({"value": [
                {"subscriptionId": SUB, "displayName": "contoso-dev", "state": "Enabled", "tenantId": "t"},
                {"subscriptionId": "00000000-0000-4000-8000-000000000002", "displayName": "contoso-prod", "state": "Enabled", "tenantId": "t"},
            ]}))],
            "[azure]\nsubscriptions = \"00000000-0000-4000-8000-000000000002\"\n",
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains(&format!("[azure] leaves out the subscription {SUB}")),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1, "no query went out");
    }
}
