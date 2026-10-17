//! `entra role list`: active directory role assignments from
//! `/roleManagement/directory/roleAssignments`, each principal expanded and
//! each role named from the role definitions (cached). A principal's list
//! adds the roles its role-assignable groups hold, `via` naming the group.
//! PIM-eligible assignments are not read.

use std::collections::HashMap;

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{guid, limited, text};
use crate::config::Azure;
use crate::entra::{Graph, Kind, V1, find, odata, param};

#[derive(clap::Args)]
pub struct RoleListArgs {
    /// Only the roles this user, group or service principal holds, through groups too: an id, UPN, name or @me
    #[arg(long)]
    principal: Option<String>,
    /// Only who holds this role: its name (Global Administrator) or id
    #[arg(long)]
    role: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RoleRow {
    /// The assignment's id.
    id: String,
    role: Option<String>,
    principal: Holder,
    /// Where it applies: / is the whole tenant.
    scope: Option<String>,
    /// The role-assignable group it is held through.
    via: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Holder {
    id: String,
    /// user, group or serviceprincipal.
    #[serde(rename = "type")]
    kind: Option<String>,
    name: Option<String>,
}

/// Every role definition's id and name, cached for `[azure] refresh`.
fn definitions(graph: &Graph<'_>) -> Result<Vec<(String, String)>> {
    let key = "entra/roledefinitions";
    let ctx = graph.ctx();
    if let Some(held) = ctx.cache().get::<Vec<(String, String)>>(key) {
        return Ok(held);
    }
    let found: Vec<(String, String)> = graph
        .pages(
            &format!("{V1}/roleManagement/directory/roleDefinitions?$select=id,displayName"),
            usize::MAX,
        )?
        .rows
        .iter()
        .filter_map(|held| {
            Some((
                text(&held["id"])?.to_ascii_lowercase(),
                text(&held["displayName"])?,
            ))
        })
        .collect();
    ctx.cache().put(key, &found, Azure::load(ctx)?.refresh());
    Ok(found)
}

fn rows(
    graph: &Graph<'_>,
    filter: Option<String>,
    limit: usize,
    names: &HashMap<String, String>,
    via: Option<&HashMap<String, String>>,
) -> Result<(Vec<RoleRow>, bool)> {
    let mut url = format!("{V1}/roleManagement/directory/roleAssignments?$expand=principal");
    if let Some(filter) = filter {
        url.push_str(&format!("&$filter={}", param(&filter)));
    }
    let found = graph.pages(&url, limit)?;
    let rows = found
        .rows
        .iter()
        .filter_map(|held: &Value| {
            let principal = &held["principal"];
            let id = text(&held["principalId"])?;
            Some(RoleRow {
                id: text(&held["id"])?,
                role: text(&held["roleDefinitionId"])
                    .and_then(|role| names.get(&role.to_ascii_lowercase()).cloned()),
                via: via.and_then(|groups| groups.get(&id.to_ascii_lowercase()).cloned()),
                principal: Holder {
                    kind: Kind::of(principal).map(|kind| kind.word().to_owned()),
                    name: text(&principal["displayName"]),
                    id,
                },
                scope: text(&held["directoryScopeId"]),
            })
        })
        .collect();
    Ok((rows, found.more))
}

fn role_list(ctx: &Ctx, args: RoleListArgs) -> Result<Vec<RoleRow>> {
    let graph = Graph::new(ctx);
    let defined = definitions(&graph)?;
    let names: HashMap<String, String> = defined.iter().cloned().collect();
    let mut filters = Vec::new();
    if let Some(role) = &args.role {
        let role = role.trim();
        let id = defined
            .iter()
            .find(|(id, name)| id.eq_ignore_ascii_case(role) || name.eq_ignore_ascii_case(role))
            .map(|(id, _)| id.clone())
            .ok_or_else(|| {
                Failure::usage(format!("no directory role is named {role:?}"))
                    .hint("`agent-cli entra role list` names the roles held")
            })?;
        filters.push(format!("roleDefinitionId eq {}", odata(&id)));
    }
    ctx.note("[active assignments only; PIM-eligible ones are not shown]");
    let Some(principal) = &args.principal else {
        let (rows, more) = rows(&graph, filters.pop(), args.limit, &names, None)?;
        if more {
            ctx.note(format!("[first {}; --limit N, or --role NAME]", args.limit));
        }
        return Ok(rows);
    };
    let (kind, held) = find(
        &graph,
        principal,
        &[Kind::User, Kind::ServicePrincipal, Kind::Group],
        "",
    )?;
    let id = text(&held["id"]).unwrap_or_default();
    let mut by_principal = filters.clone();
    by_principal.push(format!("principalId eq {}", odata(&id)));
    let (mut found, _) = rows(
        &graph,
        Some(by_principal.join(" and ")),
        usize::MAX,
        &names,
        None,
    )?;
    // VERIFY(work): that transitiveMemberOf takes isAssignableToRole in $select.
    let groups: HashMap<String, String> = graph
        .pages(
            &format!(
                "{V1}/{}/{id}/transitiveMemberOf?$select=id,displayName,isAssignableToRole",
                kind.collection()
            ),
            usize::MAX,
        )?
        .rows
        .iter()
        .filter(|group| group["isAssignableToRole"] == true)
        .filter_map(|group| {
            let id = text(&group["id"]).filter(|id| guid(id))?;
            Some((
                id.to_ascii_lowercase(),
                text(&group["displayName"]).unwrap_or(id),
            ))
        })
        .collect();
    let ids: Vec<String> = groups.keys().cloned().collect();
    // VERIFY(work): 15 is the documented cap for `in` on most properties.
    for chunk in ids.chunks(15) {
        let quoted: Vec<String> = chunk.iter().map(|id| odata(id)).collect();
        let mut through = filters.clone();
        through.push(format!("principalId in ({})", quoted.join(",")));
        let (held, _) = rows(
            &graph,
            Some(through.join(" and ")),
            usize::MAX,
            &names,
            Some(&groups),
        )?;
        found.extend(held);
    }
    Ok(limited(ctx, found, args.limit))
}

command! {
    pub ROLE_LIST = ["entra", "role", "list"], Read,
    "List Entra directory roles held: who is Global Administrator, what X holds",
    keywords: ["admin", "administrator", "global", "directory", "privileged", "reader", "holds"],
    example: "entra role list --role \"Global Administrator\" --fields role,principal.name",
    run: role_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::entra::testing::{V1, entra, page, urls};

    const GA: &str = "62e90394-69f5-4237-9190-012177145e10";
    const DANA: &str = "00000000-0000-4000-8000-00000000e101";

    #[test]
    fn a_role_by_name_lists_who_holds_it_and_a_principal_adds_its_groups_roles() {
        let definitions = || {
            page(vec![
                json!({"id": GA, "displayName": "Global Administrator"}),
                json!({"id": "r2", "displayName": "Reports Reader"}),
            ])
        };
        let (outcome, transport) = entra(
            &["entra", "role", "list", "--role", "global administrator"],
            vec![
                definitions(),
                page(vec![
                    json!({"id": "a1", "principalId": DANA, "roleDefinitionId": GA, "directoryScopeId": "/",
                    "principal": {"@odata.type": "#microsoft.graph.user", "id": DANA, "displayName": "Dana Reyes"}}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "a1", "role": "Global Administrator", "scope": "/",
                "principal": {"id": DANA, "type": "user", "name": "Dana Reyes"}}])
        );
        assert!(
            outcome.stderr.contains("PIM-eligible"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{V1}/roleManagement/directory/roleAssignments?$expand=principal&$filter=roleDefinitionId%20eq%20%27{GA}%27"
            )
        );

        let (outcome, _) = entra(
            &["entra", "role", "list", "--principal", DANA],
            vec![
                definitions(),
                page(vec![
                    json!({"@odata.type": "#microsoft.graph.user", "id": DANA}),
                ]),
                page(vec![]),
                page(vec![
                    json!({"@odata.type": "#microsoft.graph.group", "id": "00000000-0000-4000-8000-00000000f009",
                    "displayName": "sg-contoso-admins", "isAssignableToRole": true}),
                ]),
                page(vec![
                    json!({"id": "a2", "principalId": "00000000-0000-4000-8000-00000000f009", "roleDefinitionId": "r2",
                    "directoryScopeId": "/", "principal": {"@odata.type": "#microsoft.graph.group",
                    "id": "00000000-0000-4000-8000-00000000f009", "displayName": "sg-contoso-admins"}}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got[0]["role"], "Reports Reader");
        assert_eq!(got[0]["via"], "sg-contoso-admins");
    }
}
