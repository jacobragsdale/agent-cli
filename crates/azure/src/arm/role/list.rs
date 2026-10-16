//! `arm role list`: role assignments from Resource Graph, by principal or by
//! the scope they apply at (inherited ones included), with role names.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::arm::resource::any;
use crate::arm::role::{Assignment, assignments};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct RoleListArgs {
    /// Only this principal's: a user, group, service principal or managed identity's object id (arm identity get prints it as principal_id)
    #[arg(long)]
    principal: Option<String>,
    /// Only those that apply here, inherited from its group and subscription too: an ARM id, portal link or resource name
    #[arg(long)]
    scope: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn role_list(ctx: &Ctx, args: RoleListArgs) -> Result<Vec<Assignment>> {
    let azure = Azure::load(ctx)?;
    let at = match &args.scope {
        Some(raw) => Some(any(ctx, &azure, raw, None)?),
        None => None,
    };
    assignments(
        ctx,
        &azure,
        args.principal.as_deref().map(str::trim),
        at.as_ref(),
        args.limit,
    )
}

command! {
    pub ROLE_LIST = ["arm", "role", "list"], Read,
    "List Azure role assignments (RBAC) for a principal or applying at a scope",
    keywords: ["rbac", "permission", "permissions", "access", "granted", "assignment", "assignments", "iam", "authorization", "forbidden", "403"],
    example: "arm role list --scope kv-contoso-prod --fields principal_id,role,inherited",
    run: role_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{azure, inventory};

    const SUB: &str = "/subscriptions/00000000-0000-4000-8000-000000000001";

    #[test]
    fn a_scope_gets_the_assignments_granted_on_it_and_above_it_marked_inherited() {
        let vault = format!(
            "{SUB}/resourceGroups/rg-contoso/providers/Microsoft.KeyVault/vaults/kv-contoso"
        );
        let assignment = |scope: &str, role: &str| {
            json!({"id": format!("{scope}/providers/Microsoft.Authorization/roleAssignments/{role}"),
                "principalId": "00000000-0000-4000-8000-0000000000aa", "principalType": "ServicePrincipal",
                "role": role, "scope": scope})
        };
        let (outcome, transport) = azure(
            &[crate::ARM],
            &[
                "arm",
                "role",
                "list",
                "--scope",
                &vault,
                "--principal",
                "00000000-0000-4000-8000-0000000000aa",
            ],
            vec![inventory(vec![
                assignment(SUB, "Reader"),
                assignment(&vault, "Key Vault Secrets User"),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got[0]["inherited"], true, "{got}");
        assert_eq!(got[1]["inherited"], false, "{got}");
        assert_eq!(got[1]["role"], "Key Vault Secrets User");
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        let query = query.as_str().unwrap();
        assert!(query.starts_with("authorizationresources"), "{query}");
        assert!(
            query.contains("| where principalId =~ '00000000-0000-4000-8000-0000000000aa'"),
            "{query}"
        );
        assert!(
            query.contains(&format!(
                "| where scope in~ ('{SUB}', '{SUB}/resourceGroups/rg-contoso', '{vault}')"
            )),
            "{query}"
        );
    }
}
