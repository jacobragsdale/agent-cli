//! `arm resource get`: ARM `GET {id}` for any resource type, at the newest
//! stable api-version its provider lists, so no type needs code of its own.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use serde_json::Value;

use crate::arm::resource::{Lookup, fetch};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct ResourceGetArgs {
    /// The resource: its ARM id (a child collection such as SERVER_ID/firewallRules too), its portal link, or its name
    resource: String,
    /// Its type, for a name: Microsoft.Network/virtualNetworks
    #[arg(long = "type")]
    kind: Option<String>,
    /// Its resource group, for a name two groups hold
    #[arg(long)]
    group: Option<String>,
    /// Its subscription, GUID or name, for a name two subscriptions hold
    #[arg(long)]
    subscription: Option<String>,
}

fn resource_get(ctx: &Ctx, args: ResourceGetArgs) -> Result<Value> {
    let azure = Azure::load(ctx)?;
    let kinds: Vec<&str> = args.kind.iter().map(String::as_str).collect();
    let list = format!("arm resource list {}", args.resource.trim());
    let lookup = Lookup {
        kinds: &kinds,
        group: args.group.as_deref(),
        subscription: args.subscription.as_deref(),
        list: &list,
        get: "arm resource get",
    };
    fetch(ctx, &azure, &args.resource, &lookup).map(|(_, value)| value)
}

command! {
    pub RESOURCE_GET = ["arm", "resource", "get"], Read,
    "Show any Azure resource as ARM returns it, by id, portal link or name",
    keywords: ["properties", "config", "settings", "firewall", "rules", "show", "json", "arm"],
    example: "arm resource get vnet-contoso-prod --type Microsoft.Network/virtualNetworks --fields properties.subnets",
    run: resource_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{azure, azure_with, inventory};

    const SUB: &str = "00000000-0000-4000-8000-000000000001";

    fn rg(path: &str) -> String {
        format!("/subscriptions/{SUB}/resourceGroups/rg-contoso-prod{path}")
    }

    fn provider(path: &str, versions: &[&str]) -> Answer {
        Answer::json(&json!({"namespace": "Microsoft.Network", "resourceTypes": [
            {"resourceType": "virtualNetworks", "apiVersions": ["2099-01-01"]},
            {"resourceType": path, "apiVersions": versions},
        ]}))
    }

    #[test]
    fn a_nested_type_is_read_at_its_newest_stable_api_version() {
        let subnet =
            rg("/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod/subnets/aks");
        let link =
            format!("https://portal.azure.com/#@contoso.onmicrosoft.com/resource{subnet}/overview");
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", &link],
            vec![
                provider(
                    "virtualNetworks/subnets",
                    &[
                        "2024-07-01-preview",
                        "2024-05-01",
                        "2023-11-01",
                        "2024-01-01",
                    ],
                ),
                Answer::json(&json!({"id": subnet, "name": "aks",
                    "properties": {"addressPrefix": "10.1.0.0/22",
                        "keyVaultSecretId": "https://kv-contoso-prod.vault.azure.net/secrets/tls"}})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["properties"]["addressPrefix"], "10.1.0.0/22");
        assert_eq!(
            outcome.json()["properties"]["keyVaultSecretId"],
            "https://kv-contoso-prod.vault.azure.net/secrets/tls",
            "a reference to a secret is not one"
        );
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            format!(
                "https://management.azure.com/subscriptions/{SUB}/providers/Microsoft.Network?api-version=2021-04-01"
            )
        );
        assert_eq!(
            sent[1].url,
            format!("https://management.azure.com{subnet}?api-version=2024-05-01")
        );
        assert_eq!(
            sent[1].authorization.as_deref(),
            Some("Bearer token@https://management.azure.com/")
        );
    }

    #[test]
    fn a_preview_only_type_is_read_with_its_newest_preview_and_says_so() {
        let id = rg("/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod/subnets");
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", &id],
            vec![
                provider(
                    "virtualNetworks/subnets",
                    &["2024-01-01-preview", "2024-03-01-preview"],
                ),
                Answer::json(&json!({"value": []})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("only preview api-versions; read with 2024-03-01-preview"),
            "{}",
            outcome.stderr
        );
        assert!(
            transport.sent()[1]
                .url
                .ends_with("api-version=2024-03-01-preview")
        );
    }

    #[test]
    fn a_name_is_found_by_resource_graph_and_two_holders_are_listed_not_guessed() {
        let vnet = rg("/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod");
        let (outcome, transport) = azure(
            &[crate::ARM],
            &[
                "arm",
                "resource",
                "get",
                "vnet-contoso-prod",
                "--type",
                "Microsoft.Network/virtualNetworks",
            ],
            vec![
                inventory(vec![json!({ "id": vnet })]),
                provider("virtualNetworks", &["2024-05-01"]),
                Answer::json(&json!({ "id": vnet })),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert_eq!(
            query,
            "resources\n| where name =~ 'vnet-contoso-prod'\n| where type =~ 'Microsoft.Network/virtualNetworks'\n| project id\n| order by id asc"
        );

        let other = vnet.replace("rg-contoso-prod", "rg-contoso-dev");
        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", "vnet-contoso-prod"],
            vec![inventory(vec![
                json!({ "id": other }),
                json!({ "id": vnet }),
            ])],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains(&other) && outcome.stderr.contains(&vnet),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", "vnet-gone"],
            vec![inventory(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("arm resource list vnet-gone"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn an_id_the_flags_or_the_allowlist_disagree_with_is_a_usage_error() {
        let vnet = rg("/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod");
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", &vnet, "--group", "rg-contoso-dev"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());

        let (outcome, transport) = azure_with(
            &[crate::ARM],
            &["arm", "resource", "get", &vnet],
            vec![],
            "[azure]\nsubscriptions = \"00000000-0000-4000-8000-000000000002\"\n",
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("leaves out"), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());

        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", "https://example.com/x"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_missing_resource_is_exit_4_naming_the_list_to_run() {
        let vnet = rg("/providers/Microsoft.Network/virtualNetworks/vnet-gone");
        let url = format!("https://management.azure.com{vnet}?api-version=2020-01-01");
        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", &url],
            vec![
                provider("virtualNetworks", &["2024-05-01"]),
                Answer::status(
                    404,
                    r#"{"error":{"code":"ResourceNotFound","message":"The Resource 'Microsoft.Network/virtualNetworks/vnet-gone' under resource group 'rg-contoso-prod' was not found."}}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("arm resource list --type Microsoft.Network/virtualNetworks"),
            "{}",
            outcome.stderr
        );
    }
}
