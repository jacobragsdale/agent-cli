//! `arm role list`: role assignments from Resource Graph, by principal or by
//! the scope they apply at (inherited ones included), with role names and
//! each principal's name from one Microsoft Graph `getByIds` (what `az role
//! assignment list` does); a Graph refusal leaves the names out with a note.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::arm::resource::any;
use crate::arm::role::{Assignment, assignments};
use crate::client::text;
use crate::config::Azure;
use crate::entra::Graph;

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

/// One role assignment, with its principal's name.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RoleRow {
    /// The assignment's ARM id, which `arm resource get` takes.
    id: String,
    /// The user, group or service principal's object id, which entra principal list takes.
    principal_id: String,
    /// Its display name in Entra; absent when deleted (an orphaned assignment) or Graph refused.
    principal_name: Option<String>,
    /// User, Group or ServicePrincipal (a managed identity is one).
    principal_type: Option<String>,
    /// The role's name: Key Vault Secrets User.
    role: Option<String>,
    /// Where it is granted: a subscription, a group or a resource, as an ARM id.
    scope: String,
    /// With `--scope`: granted above it (its group or subscription), not on it.
    inherited: Option<bool>,
}

fn role_list(ctx: &Ctx, args: RoleListArgs) -> Result<Vec<RoleRow>> {
    let azure = Azure::load(ctx)?;
    let at = match &args.scope {
        Some(raw) => Some(any(ctx, &azure, raw, None)?),
        None => None,
    };
    let principals: Vec<&str> = args
        .principal
        .as_deref()
        .map(str::trim)
        .into_iter()
        .collect();
    let found = assignments(ctx, &azure, &principals, at.as_ref(), args.limit)?;
    let mut ids: Vec<String> = Vec::new();
    for held in &found {
        if !ids.contains(&held.principal_id) {
            ids.push(held.principal_id.clone());
        }
    }
    let names: Vec<(String, String)> = if ids.is_empty() {
        Vec::new()
    } else {
        match Graph::new(ctx).by_ids(&ids, &["user", "group", "servicePrincipal"]) {
            Ok(objects) => objects
                .iter()
                .filter_map(|held| Some((text(&held["id"])?, text(&held["displayName"])?)))
                .collect(),
            Err(error) => {
                ctx.note(format!(
                    "[principal names left out: {error:#}; `agent-cli doctor entra` shows what the login may read]"
                ));
                Vec::new()
            }
        }
    };
    Ok(found
        .into_iter()
        .map(|held: Assignment| RoleRow {
            principal_name: names
                .iter()
                .find(|(id, _)| id.eq_ignore_ascii_case(&held.principal_id))
                .map(|(_, name)| name.clone()),
            id: held.id,
            principal_id: held.principal_id,
            principal_type: held.principal_type,
            role: held.role,
            scope: held.scope,
            inherited: held.inherited,
        })
        .collect())
}
command! {
    pub ROLE_LIST = ["arm", "role", "list"], Read,
    "List Azure role assignments (RBAC) for a principal or applying at a scope",
    keywords: ["rbac", "permission", "permissions", "access", "granted", "assignment", "assignments", "iam", "authorization", "forbidden", "403", "contributor", "owner"],
    example: "arm role list --scope kv-contoso-prod --fields principal_id,role,inherited",
    run: role_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
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
            vec![
                inventory(vec![
                    assignment(SUB, "Reader"),
                    assignment(&vault, "Key Vault Secrets User"),
                ]),
                Answer::json(
                    &json!({"value": [{"@odata.type": "#microsoft.graph.servicePrincipal",
                    "id": "00000000-0000-4000-8000-0000000000aa", "displayName": "id-contoso-notifier"}]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got[0]["inherited"], true, "{got}");
        assert_eq!(got[1]["inherited"], false, "{got}");
        assert_eq!(got[1]["role"], "Key Vault Secrets User");
        assert_eq!(got[1]["principal_name"], "id-contoso-notifier");
        let names = &transport.sent()[1];
        assert_eq!(
            names.url,
            "https://graph.microsoft.com/v1.0/directoryObjects/getByIds"
        );
        assert_eq!(
            names.body.as_ref().unwrap()["ids"],
            json!(["00000000-0000-4000-8000-0000000000aa"])
        );
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

    #[test]
    fn a_graph_refusal_leaves_the_names_out_with_one_note() {
        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "role", "list"],
            vec![
                inventory(vec![
                    json!({"id": "a1", "principalId": "00000000-0000-4000-8000-0000000000aa",
                    "role": "Reader", "scope": SUB}),
                ]),
                Answer::status(
                    403,
                    r#"{"error":{"code":"Authorization_RequestDenied","message":"Insufficient privileges to complete the operation."}}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(outcome.json()[0]["principal_name"].is_null());
        assert!(
            outcome.stderr.contains("[principal names left out: ")
                && outcome.stderr.contains("`agent-cli doctor entra`"),
            "{}",
            outcome.stderr
        );
    }
}
