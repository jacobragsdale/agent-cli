//! `kv vault get`: the vault's ARM GET, read for what decides who and what
//! reaches it: RBAC or access policies, soft delete, the firewall and the
//! private endpoints, whose ids `net endpoint get` takes.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::arm::resource::{Lookup, name_in};
use crate::client::text;
use crate::config::Azure;
use crate::kv::vaults;
use crate::net::{list, read, strings};

/// A stable Microsoft.KeyVault api-version.
const API: &str = "2023-07-01";

#[derive(clap::Args)]
pub struct VaultGetArgs {
    /// The vault: its name, URI, ARM id or portal link
    vault: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct VaultRow {
    /// The vault's name, as `kv vault list` prints it.
    id: String,
    /// Its ARM id, which `arm resource get` and `arm role list --scope` take.
    arm_id: String,
    uri: Option<String>,
    location: Option<String>,
    sku: Option<String>,
    /// `rbac` (Azure roles decide; see arm role list) or `access policies`.
    authorization: String,
    /// Under access policies: each principal's permissions.
    access_policies: Vec<Policy>,
    soft_delete: bool,
    soft_delete_days: Option<u64>,
    purge_protection: bool,
    /// Enabled or Disabled: whether anything outside a private endpoint gets in.
    public_network_access: Option<String>,
    network: Network,
    /// Its private endpoints.
    endpoints: Vec<Endpoint>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Policy {
    /// The principal's Entra object id.
    object_id: Option<String>,
    secrets: Vec<String>,
    keys: Vec<String>,
    certificates: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Network {
    /// Allow or Deny: what happens to a caller no rule names.
    default_action: Option<String>,
    /// AzureServices or None: whether trusted Azure services pass the firewall.
    bypass: Option<String>,
    /// The address ranges let through, as CIDRs.
    allowed_ranges: Vec<String>,
    /// Subnet ARM ids, which `net subnet get` takes.
    vnet_rules: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Endpoint {
    /// The private endpoint's ARM id, which `net endpoint get` takes.
    id: Option<String>,
    /// Approved, Pending, Rejected or Disconnected.
    status: Option<String>,
}

fn vault_get(ctx: &Ctx, args: VaultGetArgs) -> Result<VaultRow> {
    let azure = Azure::load(ctx)?;
    let raw = args.vault.trim();
    // The vault's own URI names it in its host.
    let host = raw
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .map(str::to_ascii_lowercase);
    let name = match host
        .as_deref()
        .and_then(|h| h.strip_suffix(".vault.azure.net"))
    {
        Some(name) => name.to_owned(),
        None => name_in(
            ctx,
            &azure,
            raw,
            &Lookup {
                kinds: &["Microsoft.KeyVault/vaults"],
                group: None,
                subscription: None,
                list: "kv vault list",
                get: "kv vault get",
            },
        )?,
    };
    let vault = vaults(ctx, &azure, &[name])?.remove(0);
    let arm_id = format!(
        "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.KeyVault/vaults/{}",
        vault.subscription, vault.resource_group, vault.name
    );
    let answer = read(ctx, &arm_id, API, "kv vault list")?;
    Ok(shape(vault.name, arm_id, &answer))
}

fn shape(id: String, arm_id: String, answer: &Value) -> VaultRow {
    let props = &answer["properties"];
    let rbac = props["enableRbacAuthorization"].as_bool() == Some(true);
    let acls = &props["networkAcls"];
    VaultRow {
        id,
        arm_id,
        uri: text(&props["vaultUri"]),
        location: text(&answer["location"]),
        sku: text(&props["sku"]["name"]),
        authorization: if rbac { "rbac" } else { "access policies" }.to_owned(),
        // A vault in RBAC mode keeps its old policies but ignores them.
        access_policies: if rbac {
            Vec::new()
        } else {
            list(&props["accessPolicies"])
                .iter()
                .map(|policy| {
                    let held = &policy["permissions"];
                    Policy {
                        object_id: text(&policy["objectId"]),
                        secrets: strings(&held["secrets"]),
                        keys: strings(&held["keys"]),
                        certificates: strings(&held["certificates"]),
                    }
                })
                .collect()
        },
        // Soft delete is on in every vault made since 2020 and cannot be
        // turned off; only an older vault says false.
        soft_delete: props["enableSoftDelete"].as_bool().unwrap_or(true),
        soft_delete_days: props["softDeleteRetentionInDays"].as_u64(),
        purge_protection: props["enablePurgeProtection"].as_bool() == Some(true),
        public_network_access: text(&props["publicNetworkAccess"]),
        network: Network {
            // No networkAcls at all is a vault open to every network.
            default_action: text(&acls["defaultAction"]).or_else(|| Some("Allow".to_owned())),
            bypass: text(&acls["bypass"]),
            allowed_ranges: list(&acls["ipRules"])
                .iter()
                .filter_map(|rule| text(&rule["value"]))
                .collect(),
            vnet_rules: list(&acls["virtualNetworkRules"])
                .iter()
                .filter_map(|rule| text(&rule["id"]))
                .collect(),
        },
        endpoints: list(&props["privateEndpointConnections"])
            .iter()
            .map(|connection| {
                let held = &connection["properties"];
                Endpoint {
                    id: text(&held["privateEndpoint"]["id"]),
                    status: text(&held["privateLinkServiceConnectionState"]["status"]),
                }
            })
            .collect(),
    }
}

command! {
    pub VAULT_GET = ["kv", "vault", "get"], Read,
    "Show a key vault's access model, network rules, endpoints and soft delete",
    keywords: ["rbac", "access", "policy", "policies", "acl", "purge", "protection", "deny", "default"],
    example: "kv vault get kv-contoso-prod --fields authorization,network,endpoints",
    run: vault_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::KV;
    use crate::testing::{self, RG, azure};

    const ID: &str = "/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-contoso/providers/Microsoft.KeyVault/vaults/kv-contoso-dev";

    fn arm(rbac: bool) -> Answer {
        Answer::json(&json!({
            "id": ID, "name": "kv-contoso-dev", "location": "eastus",
            "properties": {
                "vaultUri": "https://kv-contoso-dev.vault.azure.net/", "sku": {"family": "A", "name": "standard"},
                "enableRbacAuthorization": rbac, "enableSoftDelete": true, "softDeleteRetentionInDays": 90,
                "enablePurgeProtection": true, "publicNetworkAccess": "Disabled",
                "accessPolicies": [{"tenantId": "t", "objectId": "00000000-0000-4000-8000-0000000000c1",
                    "permissions": {"secrets": ["get", "list"], "keys": [], "certificates": ["get"]}}],
                "networkAcls": {"bypass": "AzureServices", "defaultAction": "Deny",
                    "ipRules": [{"value": "203.0.113.0/24"}],
                    "virtualNetworkRules": [{"id": format!("{RG}/providers/Microsoft.Network/virtualNetworks/vnet/subnets/aks")}]},
                "privateEndpointConnections": [{"id": format!("{ID}/privateEndpointConnections/pe-1"),
                    "properties": {"privateEndpoint": {"id": format!("{RG}/providers/Microsoft.Network/privateEndpoints/pe-kv")},
                        "privateLinkServiceConnectionState": {"status": "Approved"}}}],
            },
        }))
    }

    #[test]
    fn a_vault_by_name_uri_or_id_reads_its_arm_get() {
        for raw in [
            "kv-contoso-dev",
            "https://kv-contoso-dev.vault.azure.net/",
            ID,
        ] {
            let (outcome, transport) = azure(
                &[KV],
                &["kv", "vault", "get", raw],
                vec![
                    testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                    arm(true),
                ],
            );
            assert_eq!(outcome.code, 0, "{raw}: {outcome:?}");
            assert_eq!(
                transport.sent()[1].url,
                format!("https://management.azure.com{ID}?api-version=2023-07-01")
            );
            let row = outcome.json();
            assert_eq!(row["id"], "kv-contoso-dev");
            assert_eq!(row["authorization"], "rbac");
            assert_eq!(row.get("access_policies"), None, "ignored under RBAC");
            assert_eq!(
                row["network"],
                json!({"default_action": "Deny", "bypass": "AzureServices",
                    "allowed_ranges": ["203.0.113.0/24"],
                    "vnet_rules": [format!("{RG}/providers/Microsoft.Network/virtualNetworks/vnet/subnets/aks")]})
            );
            assert_eq!(
                row["endpoints"],
                json!([{"id": format!("{RG}/providers/Microsoft.Network/privateEndpoints/pe-kv"), "status": "Approved"}])
            );
            assert_eq!(row["purge_protection"], true);
        }
    }

    #[test]
    fn access_policies_are_summarised_by_object_id() {
        let (outcome, _) = azure(
            &[KV],
            &[
                "kv",
                "vault",
                "get",
                "kv-contoso-dev",
                "--fields",
                "authorization,access_policies",
            ],
            vec![
                testing::inventory(vec![testing::vault("kv-contoso-dev")]),
                arm(false),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"authorization": "access policies", "access_policies": [{
                "object_id": "00000000-0000-4000-8000-0000000000c1",
                "secrets": ["get", "list"], "certificates": ["get"]}]})
        );
    }

    #[test]
    fn a_vault_outside_the_allowlist_or_another_type_is_refused() {
        let (outcome, transport) = azure(
            &[KV],
            &[
                "kv",
                "vault",
                "get",
                &format!("{RG}/providers/Microsoft.Network/virtualNetworks/vnet"),
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
        let (outcome, _) = azure(
            &[KV],
            &["kv", "vault", "get", "kv-gone"],
            vec![testing::inventory(vec![testing::vault("kv-contoso-dev")])],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("the login reaches no vault kv-gone"),
            "{}",
            outcome.stderr
        );
    }
}
