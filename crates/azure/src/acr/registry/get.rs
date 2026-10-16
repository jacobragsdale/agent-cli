//! `acr registry get`: the registry's ARM GET, its webhooks and, on a
//! Premium registry (the only tier that has them), its replications: who
//! reaches it (admin user, public access, network rules, private endpoints)
//! and what it keeps (retention).

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::acr::{API, arm_id, named};
use crate::client::text;
use crate::config::Azure;
use crate::net::{list, read, strings};

#[derive(clap::Args)]
pub struct RegistryGetArgs {
    /// The registry: its name, login server, ARM id or portal link
    registry: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RegistryRow {
    /// The registry's name, as `acr registry list` prints it.
    id: String,
    /// Its ARM id, which `arm resource get` and `arm role list --scope` take.
    arm_id: String,
    login_server: String,
    location: Option<String>,
    /// Basic, Standard or Premium.
    sku: Option<String>,
    /// Whether the shared admin user and its passwords work.
    admin_user: bool,
    anonymous_pull: bool,
    /// Enabled or Disabled.
    public_network_access: Option<String>,
    network: Network,
    /// Its private endpoints.
    endpoints: Vec<Endpoint>,
    /// Days an untagged manifest is kept, when the retention policy is on.
    retention_days: Option<u64>,
    zone_redundancy: Option<String>,
    replications: Vec<Replication>,
    webhooks: Vec<Webhook>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Network {
    /// Allow or Deny (Premium only): what happens to a caller no rule names.
    default_action: Option<String>,
    /// The address ranges let through, as CIDRs.
    allowed_ranges: Vec<String>,
    /// AzureServices or None: whether trusted Azure services get past it.
    bypass: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Endpoint {
    /// The private endpoint's ARM id, which `net endpoint get` takes.
    id: Option<String>,
    /// Approved, Pending, Rejected or Disconnected.
    status: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Replication {
    name: Option<String>,
    location: Option<String>,
    /// Online, Syncing or Offline.
    status: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Webhook {
    name: Option<String>,
    /// enabled or disabled.
    status: Option<String>,
    /// push, delete, quarantine, chart_push, chart_delete.
    actions: Vec<String>,
    /// The repositories and tags it fires for; empty is all of them.
    scope: Option<String>,
}

fn registry_get(ctx: &Ctx, args: RegistryGetArgs) -> Result<RegistryRow> {
    let azure = Azure::load(ctx)?;
    let registry = named(ctx, &azure, &args.registry)?;
    let id = arm_id(&registry);
    let answer = read(ctx, &id, API, "acr registry list")?;
    let props = &answer["properties"];
    let sku = text(&answer["sku"]["name"]);
    let replications = if sku.as_deref() == Some("Premium") {
        let held = read(ctx, &format!("{id}/replications"), API, "acr registry list")?;
        list(&held["value"])
            .iter()
            .map(|replica| Replication {
                name: text(&replica["name"]),
                location: text(&replica["location"]),
                status: text(&replica["properties"]["status"]["displayStatus"]),
            })
            .collect()
    } else {
        Vec::new()
    };
    let hooks = read(ctx, &format!("{id}/webhooks"), API, "acr registry list")?;
    let rules = &props["networkRuleSet"];
    let retention = &props["policies"]["retentionPolicy"];
    Ok(RegistryRow {
        id: registry.name.clone(),
        arm_id: id,
        login_server: registry.login_server.clone(),
        location: text(&answer["location"]),
        sku,
        admin_user: props["adminUserEnabled"].as_bool() == Some(true),
        anonymous_pull: props["anonymousPullEnabled"].as_bool() == Some(true),
        public_network_access: text(&props["publicNetworkAccess"]),
        network: Network {
            default_action: text(&rules["defaultAction"]),
            allowed_ranges: list(&rules["ipRules"])
                .iter()
                .filter_map(|rule| text(&rule["value"]))
                .collect(),
            bypass: text(&props["networkRuleBypassOptions"]),
        },
        endpoints: list(&props["privateEndpointConnections"])
            .iter()
            .map(endpoint)
            .collect(),
        retention_days: (text(&retention["status"]).as_deref() == Some("enabled"))
            .then(|| retention["days"].as_u64())
            .flatten(),
        zone_redundancy: text(&props["zoneRedundancy"]),
        replications,
        webhooks: list(&hooks["value"])
            .iter()
            .map(|hook| Webhook {
                name: text(&hook["name"]),
                status: text(&hook["properties"]["status"]),
                actions: strings(&hook["properties"]["actions"]),
                scope: text(&hook["properties"]["scope"]),
            })
            .collect(),
    })
}

fn endpoint(connection: &Value) -> Endpoint {
    let held = &connection["properties"];
    Endpoint {
        id: text(&held["privateEndpoint"]["id"]),
        status: text(&held["privateLinkServiceConnectionState"]["status"]),
    }
}

command! {
    pub REGISTRY_GET = ["acr", "registry", "get"], Read,
    "Show a container registry's SKU, access, network rules, retention and webhooks",
    keywords: ["admin", "anonymous", "network", "public", "retention", "replication", "geo", "webhook", "sku"],
    example: "acr registry get contosoacr --fields sku,admin_user,network,retention_days",
    run: registry_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::ACR;
    use crate::testing::{RG, azure, one_registry};

    fn registry(sku: &str) -> Answer {
        Answer::json(&json!({
            "name": "contosoacr", "location": "eastus", "sku": {"name": sku},
            "properties": {"loginServer": "contosoacr.azurecr.io", "adminUserEnabled": false,
                "publicNetworkAccess": "Enabled", "networkRuleBypassOptions": "AzureServices",
                "networkRuleSet": {"defaultAction": "Deny", "ipRules": [{"action": "Allow", "value": "198.51.100.0/24"}]},
                "policies": {"retentionPolicy": {"days": 7, "status": "enabled"}},
                "privateEndpointConnections": [], "zoneRedundancy": "Disabled"},
        }))
    }

    #[test]
    fn a_premium_registry_reads_its_replications_and_webhooks() {
        let id = format!("{RG}/providers/Microsoft.ContainerRegistry/registries/contosoacr");
        for raw in ["contosoacr", "https://contosoacr.azurecr.io", id.as_str()] {
            let (outcome, transport) = azure(
                &[ACR],
                &["acr", "registry", "get", raw],
                vec![
                    one_registry(),
                    registry("Premium"),
                    Answer::json(
                        &json!({"value": [{"name": "westeurope", "location": "westeurope",
                        "properties": {"status": {"displayStatus": "Ready"}}}]}),
                    ),
                    Answer::json(&json!({"value": [{"name": "deploy", "properties": {
                        "status": "enabled", "actions": ["push"], "scope": "api:*"}}]})),
                ],
            );
            assert_eq!(outcome.code, 0, "{raw}: {outcome:?}");
            let urls: Vec<String> = transport.sent().iter().map(|s| s.url.clone()).collect();
            assert_eq!(
                urls[1..],
                [
                    format!("https://management.azure.com{id}?api-version=2023-07-01"),
                    format!("https://management.azure.com{id}/replications?api-version=2023-07-01"),
                    format!("https://management.azure.com{id}/webhooks?api-version=2023-07-01"),
                ]
            );
            let row = outcome.json();
            assert_eq!(row["id"], "contosoacr");
            assert_eq!(row["retention_days"], 7);
            assert_eq!(
                row["network"],
                json!({"default_action": "Deny", "allowed_ranges": ["198.51.100.0/24"], "bypass": "AzureServices"})
            );
            assert_eq!(row["replications"][0]["status"], "Ready");
            assert_eq!(row["webhooks"][0]["scope"], "api:*");
        }
    }

    #[test]
    fn a_standard_registry_has_no_replications_to_read() {
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "registry", "get", "contosoacr"],
            vec![
                one_registry(),
                registry("Standard"),
                Answer::json(&json!({"value": []})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(transport.sent().len(), 3);
        assert_eq!(outcome.json().get("replications"), None);
    }
}
