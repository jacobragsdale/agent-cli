//! Role assignments, which `role list` and `identity get` share: Resource
//! Graph's `authorizationresources`, one query across the subscriptions in
//! scope, with each assignment's role name joined from the role definitions.

pub(crate) mod list;

use agent_cli_core::Ctx;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::arm::quote;
use crate::arm::resource::Target;
use crate::client::text;
use crate::config::Azure;
use crate::graph::rows;

/// One role assignment.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Assignment {
    /// The assignment's ARM id, which `arm resource get` takes.
    pub id: String,
    /// The user, group or service principal's object id.
    pub principal_id: String,
    /// User, Group or ServicePrincipal (a managed identity is one).
    pub principal_type: Option<String>,
    /// The role's name: Key Vault Secrets User.
    pub role: Option<String>,
    /// The role definition's GUID, lowercase, which names a built-in role
    /// whatever its display name (`lake access get` matches data roles by it).
    #[serde(skip)]
    pub role_id: String,
    /// Where it is granted: a subscription, a group or a resource, as an ARM id.
    pub scope: String,
    /// With `--scope`: granted above it (its group or subscription), not on it.
    pub inherited: Option<bool>,
}

/// The ids an assignment applying at `target` may be granted at: its
/// subscription, its group, and the resource and each parent resource.
// VERIFY(work): whether management-group assignments show up in
// authorizationresources for a subscription-scoped query; if they do, add the
// subscription's managementGroupAncestorsChain here.
fn ancestors(target: &Target) -> Vec<String> {
    let segments: Vec<&str> = target.id.trim_start_matches('/').split('/').collect();
    (2..=segments.len())
        .step_by(2)
        .filter(|&end| !segments[end - 2].eq_ignore_ascii_case("providers"))
        .map(|end| format!("/{}", segments[..end].join("/")))
        .collect()
}

/// The assignments of `principals` (object ids; none is everyone's), those
/// that apply at `at`, or every one, first by scope, at most `limit`.
pub(crate) fn assignments(
    ctx: &Ctx,
    azure: &Azure,
    principals: &[&str],
    at: Option<&Target>,
    limit: usize,
) -> Result<Vec<Assignment>> {
    let mut kql = String::from(
        "authorizationresources\
         \n| where type =~ 'microsoft.authorization/roleassignments'\
         \n| extend principalId = tostring(properties.principalId), principalType = tostring(properties.principalType), \
         scope = tostring(properties.scope), roleId = tolower(extract('[^/]+$', 0, tostring(properties.roleDefinitionId)))",
    );
    match principals {
        [] => {}
        [one] => kql.push_str(&format!("\n| where principalId =~ {}", quote(one))),
        many => {
            let quoted: Vec<String> = many.iter().map(|id| quote(id)).collect();
            kql.push_str(&format!(
                "\n| where principalId in~ ({})",
                quoted.join(", ")
            ));
        }
    }
    if let Some(target) = at {
        let quoted: Vec<String> = ancestors(target).iter().map(|id| quote(id)).collect();
        kql.push_str(&format!("\n| where scope in~ ({})", quoted.join(", ")));
    }
    // VERIFY(work): that built-in role definitions are in authorizationresources
    // under their GUID as `name`, so the join names every role.
    kql.push_str(
        "\n| join kind=leftouter (authorizationresources \
         | where type =~ 'microsoft.authorization/roledefinitions' \
         | project roleId = tolower(name), role = tostring(properties.roleName)) on roleId\
         \n| project id, principalId, principalType, role, roleId, scope\
         \n| order by scope asc, id asc",
    );
    Ok(rows(ctx, &azure.subscriptions, &kql, limit)?
        .iter()
        .map(|row| {
            let scope = text(&row["scope"]).unwrap_or_default();
            Assignment {
                id: text(&row["id"]).unwrap_or_default(),
                principal_id: text(&row["principalId"]).unwrap_or_default(),
                principal_type: text(&row["principalType"]),
                role: text(&row["role"]),
                role_id: text(&row["roleId"]).unwrap_or_default(),
                inherited: at.map(|target| !scope.eq_ignore_ascii_case(&target.id)),
                scope,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arm::resource::target;

    #[test]
    fn a_resource_inherits_from_its_parents_its_group_and_its_subscription() {
        let sub = "/subscriptions/00000000-0000-4000-8000-000000000001";
        let subnet = format!(
            "{sub}/resourceGroups/rg-contoso/providers/Microsoft.Network/virtualNetworks/vnet/subnets/aks"
        );
        assert_eq!(
            ancestors(&target(&subnet).unwrap()),
            [
                sub.to_owned(),
                format!("{sub}/resourceGroups/rg-contoso"),
                format!(
                    "{sub}/resourceGroups/rg-contoso/providers/Microsoft.Network/virtualNetworks/vnet"
                ),
                subnet.clone(),
            ]
        );
        assert_eq!(ancestors(&target(sub).unwrap()), [sub]);
    }
}
