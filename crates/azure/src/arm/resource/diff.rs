//! `arm resource diff`: two resources of any type (staging's NSG against
//! prod's, two storage accounts' network rules, two clusters' node pools) as
//! ARM answers them, rules and pools matched by name, ARM's bookkeeping left
//! out.

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Side, command, diff};
use anyhow::Result;

use crate::arm::resource::{Lookup, fetch};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct ResourceDiffArgs {
    /// One side: an ARM id, its portal link, or its name
    left: String,
    /// The other side, as left
    right: String,
    /// Their type, for names: Microsoft.Network/networkSecurityGroups
    #[arg(long = "type")]
    kind: Option<String>,
    #[command(flatten)]
    diff: DiffArgs,
}

/// The lists of the resources diffs are asked of, by what names an item;
/// and what ARM stamps on every resource.
const RULES: Rules = Rules {
    keys: &[
        ("properties.securityRules", "name"),
        ("properties.defaultSecurityRules", "name"),
        ("properties.subnets", "name"),
        ("properties.agentPoolProfiles", "name"),
        ("properties.networkAcls.ipRules", "value"),
        ("properties.networkAcls.virtualNetworkRules", "id"),
        ("properties.accessPolicies", "objectId"),
        ("properties.privateEndpointConnections", "name"),
        ("properties.frontendIPConfigurations", "name"),
        ("properties.backendAddressPools", "name"),
        ("properties.loadBalancingRules", "name"),
        ("properties.probes", "name"),
    ],
    noise: &[
        "id",
        "name",
        "etag",
        "systemData",
        "properties.provisioningState",
        "properties.resourceGuid",
    ],
    ..Rules::PLAIN
};

fn resource_diff(ctx: &Ctx, args: ResourceDiffArgs) -> Result<Diff> {
    let azure = Azure::load(ctx)?;
    let kinds: Vec<&str> = args.kind.iter().map(String::as_str).collect();
    let side = |raw: &str| -> Result<Side> {
        let list = format!("arm resource list {}", raw.trim());
        let lookup = Lookup {
            kinds: &kinds,
            group: None,
            subscription: None,
            list: &list,
            get: "arm resource diff",
        };
        let (target, value) = fetch(ctx, &azure, raw, &lookup)?;
        Ok(Side::new(target.id, value))
    };
    let (left, right) = (side(&args.left)?, side(&args.right)?);
    diff(ctx, &args.diff, &RULES, left, right)
}

command! {
    pub RESOURCE_DIFF = ["arm", "resource", "diff"], Read,
    "Compare two Azure resources of any type: NSG rules, network rules, node pools",
    keywords: ["staging", "nsg", "security", "group", "firewall", "storage", "account", "pools"],
    example: "arm resource diff nsg-contoso-staging nsg-contoso-prod --type Microsoft.Network/networkSecurityGroups",
    run: resource_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::azure;

    const SUB: &str = "00000000-0000-4000-8000-000000000001";

    fn nsg(env: &str, port: &str) -> serde_json::Value {
        json!({"id": format!("/subscriptions/{SUB}/resourceGroups/rg-contoso-{env}/providers/Microsoft.Network/networkSecurityGroups/nsg-contoso-{env}"),
        "name": format!("nsg-contoso-{env}"), "etag": format!("W/\"{env}\""), "location": "westeurope",
        "properties": {"provisioningState": "Succeeded", "resourceGuid": env, "securityRules": [
            {"name": "allow-https", "properties": {"access": "Allow", "destinationPortRange": "443", "priority": 100}},
            {"name": "allow-db", "properties": {"access": "Allow", "destinationPortRange": port, "priority": 110}},
        ]}})
    }

    #[test]
    fn two_nsgs_differ_by_a_rule_not_by_their_names_or_etags() {
        let provider = || {
            Answer::json(&json!({"namespace": "Microsoft.Network", "resourceTypes": [
                {"resourceType": "networkSecurityGroups", "apiVersions": ["2024-05-01"]}]}))
        };
        let id = |env: &str| {
            format!(
                "/subscriptions/{SUB}/resourceGroups/rg-contoso-{env}/providers/Microsoft.Network/networkSecurityGroups/nsg-contoso-{env}"
            )
        };
        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "resource", "diff", &id("staging"), &id("prod")],
            vec![
                provider(),
                Answer::json(&nsg("staging", "5432")),
                provider(),
                Answer::json(&nsg("prod", "1433")),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["changes"],
            json!([{"path": "properties.securityRules.allow-db.properties.destinationPortRange",
                "left": "5432", "right": "1433"}])
        );
    }
}
