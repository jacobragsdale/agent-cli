//! `arm group list`: Resource Graph's `resourcecontainers`, one query across
//! every subscription in scope rather than a call per subscription.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::arm::scope;
use crate::client::text;
use crate::config::Azure;
use crate::graph::rows;

const QUERY: &str = "resourcecontainers
| where type =~ 'microsoft.resources/subscriptions/resourcegroups'
| project id, name, subscriptionId, location, tags
| order by name asc, id asc";

#[derive(clap::Args)]
pub struct GroupListArgs {
    /// Only this subscription: its GUID or name (within [azure] subscriptions)
    #[arg(long)]
    subscription: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GroupRow {
    /// The group's ARM id, which `arm resource get` takes.
    id: String,
    name: String,
    /// The subscription's GUID.
    subscription: String,
    location: String,
    tags: BTreeMap<String, String>,
}

fn group_list(ctx: &Ctx, args: GroupListArgs) -> Result<Vec<GroupRow>> {
    let azure = Azure::load(ctx)?;
    let scope = scope(ctx, &azure, args.subscription.as_deref())?;
    Ok(rows(ctx, &scope, QUERY, args.limit)?
        .into_iter()
        .map(|row| GroupRow {
            id: text(&row["id"]).unwrap_or_default(),
            name: text(&row["name"]).unwrap_or_default(),
            subscription: text(&row["subscriptionId"]).unwrap_or_default(),
            location: text(&row["location"]).unwrap_or_default(),
            tags: serde_json::from_value(row["tags"].clone()).unwrap_or_default(),
        })
        .collect())
}

command! {
    pub GROUP_LIST = ["arm", "group", "list"], Read,
    "List Azure resource groups across the subscriptions in scope, with their tags",
    keywords: ["resourcegroup", "rg", "groups", "subscription"],
    example: "arm group list --fields id,location,tags",
    run: group_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{azure, inventory};

    #[test]
    fn groups_come_from_one_resource_graph_query_with_the_limit_noted() {
        let group = |name: &str| {
            json!({"id": format!("/subscriptions/00000000-0000-4000-8000-000000000001/resourceGroups/{name}"),
                "name": name, "subscriptionId": "00000000-0000-4000-8000-000000000001",
                "location": "westeurope", "tags": {"env": "prod"}})
        };
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "group", "list", "--limit", "1"],
            vec![inventory(vec![
                group("rg-contoso-dev"),
                group("rg-contoso-prod"),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "/subscriptions/00000000-0000-4000-8000-000000000001/resourceGroups/rg-contoso-dev",
                "name": "rg-contoso-dev", "subscription": "00000000-0000-4000-8000-000000000001",
                "location": "westeurope", "tags": {"env": "prod"}}])
        );
        assert!(
            outcome.stderr.contains("[1 of 2; --limit N]"),
            "{}",
            outcome.stderr
        );
        let body = transport.sent()[0].body.clone().unwrap();
        assert!(
            body["query"]
                .as_str()
                .unwrap()
                .starts_with("resourcecontainers")
        );
        assert_eq!(body["options"]["$top"], 2);
        assert!(body.get("subscriptions").is_none(), "{body}");
    }
}
