//! `arm policy list`: non-compliant policy states from Resource Graph, one
//! query across the subscriptions in scope, each with its definition's display
//! name and its assignment's non-compliance message as the reason.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::arm::quote;
use crate::arm::resource::any;
use crate::client::text;
use crate::config::Azure;
use crate::graph::rows;

#[derive(clap::Args)]
pub struct PolicyListArgs {
    /// Only this resource's: an ARM id, portal link or name
    #[arg(long)]
    resource: Option<String>,
    /// Only resources in this resource group
    #[arg(long)]
    group: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PolicyRow {
    /// The non-compliant resource's ARM id.
    resource: String,
    /// The policy definition's name (a GUID for a built-in).
    policy: Option<String>,
    display_name: Option<String>,
    /// audit, deny, modify, deployIfNotExists …: deny is what refuses a
    /// request with RequestDisallowedByPolicy.
    effect: Option<String>,
    /// The assignment's non-compliance message.
    reason: Option<String>,
    /// The policy assignment's ARM id, which `arm resource get` takes.
    assignment: Option<String>,
}

fn kql(resource: Option<&str>, group: Option<&str>) -> String {
    let mut kql = String::from(
        "policyresources\
         \n| where type =~ 'microsoft.policyinsights/policystates'\
         \n| where tostring(properties.complianceState) =~ 'NonCompliant'\
         \n| extend resource = tostring(properties.resourceId), definition = tolower(tostring(properties.policyDefinitionId)), \
         assignment = tolower(tostring(properties.policyAssignmentId))",
    );
    if let Some(id) = resource {
        kql.push_str(&format!("\n| where resource =~ {}", quote(id)));
    }
    if let Some(group) = group {
        kql.push_str(&format!(
            "\n| where tostring(properties.resourceGroup) =~ {}",
            quote(group)
        ));
    }
    // VERIFY(work): that built-in policy definitions are in policyresources,
    // so the join names them; a missing display name is the sign.
    kql.push_str(
        "\n| join kind=leftouter (policyresources | where type =~ 'microsoft.authorization/policydefinitions' \
         | project definition = tolower(id), displayName = tostring(properties.displayName)) on definition\
         \n| join kind=leftouter (policyresources | where type =~ 'microsoft.authorization/policyassignments' \
         | project assignment = tolower(id), assignmentId = id, reason = tostring(properties.nonComplianceMessages[0].message)) on assignment\
         \n| project id, resource, policy = tostring(properties.policyDefinitionName), displayName, \
         effect = tostring(properties.policyDefinitionAction), reason, assignmentId\
         \n| order by resource asc, id asc",
    );
    kql
}

fn policy_list(ctx: &Ctx, args: PolicyListArgs) -> Result<Vec<PolicyRow>> {
    let azure = Azure::load(ctx)?;
    let target = match &args.resource {
        Some(raw) => Some(any(ctx, &azure, raw, args.group.as_deref())?),
        None => None,
    };
    let group = if target.is_some() {
        None
    } else {
        args.group.as_deref()
    };
    let kql = kql(target.as_ref().map(|t| t.id.as_str()), group);
    Ok(rows(ctx, &azure.subscriptions, &kql, args.limit)?
        .iter()
        .map(|row| PolicyRow {
            resource: text(&row["resource"]).unwrap_or_default(),
            policy: text(&row["policy"]),
            display_name: text(&row["displayName"]),
            effect: text(&row["effect"]),
            reason: text(&row["reason"]),
            assignment: text(&row["assignmentId"]),
        })
        .collect())
}

command! {
    pub POLICY_LIST = ["arm", "policy", "list"], Read,
    "List Azure Policy violations: non-compliant resources, the policy, effect, why",
    keywords: ["compliance", "noncompliant", "compliant", "denied", "deny", "disallowed", "RequestDisallowedByPolicy", "governance", "audit", "violation", "blocks", "blocked"],
    example: "arm policy list --group rg-contoso-prod --fields resource,display_name,effect",
    run: policy_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{azure, inventory};

    #[test]
    fn non_compliant_states_carry_the_display_name_effect_and_reason() {
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "policy", "list", "--group", "rg-contoso"],
            vec![inventory(vec![json!({"id": "s1",
                "resource": "/subscriptions/00000000-0000-4000-8000-000000000001/resourcegroups/rg-contoso/providers/microsoft.keyvault/vaults/kv-contoso",
                "policy": "405c5871-3e91-4644-8a63-58e19d68ff5b",
                "displayName": "Azure Key Vault should disable public network access",
                "effect": "audit", "reason": "Key vaults must be reached through private endpoints",
                "assignmentId": "/subscriptions/00000000-0000-4000-8000-000000000001/providers/Microsoft.Authorization/policyAssignments/kv-private"})])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["effect"], "audit");
        assert_eq!(
            outcome.json()[0]["reason"],
            "Key vaults must be reached through private endpoints"
        );
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert!(
            query
                .as_str()
                .unwrap()
                .contains("| where tostring(properties.resourceGroup) =~ 'rg-contoso'"),
            "{query}"
        );
    }
}
