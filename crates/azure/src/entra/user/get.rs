//! `entra user get`: one user, their manager, and every group and directory
//! role they hold, nested ones included (`transitiveMemberOf`, which a
//! member may read without a role-management permission).

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{stamp, text};
use crate::entra::{Graph, Kind, Named, V1, find, named};

const SELECT: &str = "id,displayName,userPrincipalName,mail,userType,accountEnabled,createdDateTime,onPremisesSyncEnabled,department,jobTitle";

#[derive(clap::Args)]
pub struct UserGetArgs {
    /// The user: object id, UPN, mail (as ado person list prints it), display name, portal link, or @me
    user: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct UserDetail {
    id: String,
    name: Option<String>,
    upn: Option<String>,
    mail: Option<String>,
    /// Member or Guest.
    #[serde(rename = "type")]
    kind: Option<String>,
    enabled: Option<bool>,
    created: Option<String>,
    /// Synced from an on-premises directory, where it is changed.
    synced: Option<bool>,
    department: Option<String>,
    job_title: Option<String>,
    manager: Option<Manager>,
    /// Every group they are in, nested ones too.
    groups: Vec<Named>,
    /// The directory roles they hold (Global Administrator …), active ones.
    roles: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Manager {
    id: String,
    name: Option<String>,
    upn: Option<String>,
}

fn user_get(ctx: &Ctx, args: UserGetArgs) -> Result<UserDetail> {
    let graph = Graph::new(ctx);
    let (_, user) = find(&graph, &args.user, &[Kind::User], SELECT)?;
    let id = text(&user["id"]).unwrap_or_default();
    let manager = match graph.get(&format!(
        "{V1}/users/{id}/manager?$select=id,displayName,userPrincipalName"
    )) {
        Ok(held) => text(&held["id"]).map(|id| Manager {
            id,
            name: text(&held["displayName"]),
            upn: text(&held["userPrincipalName"]),
        }),
        Err(error) if agent_cli_core::status_of(&error) == Some(404) => None,
        Err(error) => return Err(error),
    };
    let held = graph.pages(
        &format!("{V1}/users/{id}/transitiveMemberOf?$select=id,displayName"),
        usize::MAX,
    )?;
    let roles = held
        .rows
        .iter()
        .filter(|row| row["@odata.type"] == "#microsoft.graph.directoryRole")
        .filter_map(|row| text(&row["displayName"]))
        .collect();
    Ok(UserDetail {
        id,
        name: text(&user["displayName"]),
        upn: text(&user["userPrincipalName"]),
        mail: text(&user["mail"]),
        kind: text(&user["userType"]),
        enabled: user["accountEnabled"].as_bool(),
        created: stamp(&user["createdDateTime"]),
        synced: user["onPremisesSyncEnabled"].as_bool(),
        department: text(&user["department"]),
        job_title: text(&user["jobTitle"]),
        manager,
        groups: named(&held.rows, Kind::Group),
        roles,
    })
}

command! {
    pub USER_GET = ["entra", "user", "get"], Read,
    "Show an Entra user: UPN, enabled, manager, groups (nested) and directory roles",
    keywords: ["person", "account", "upn", "email", "disabled", "manager", "memberof", "who"],
    example: "entra user get sam@contoso.example --fields upn,enabled,groups,roles",
    run: user_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::entra::testing::{V1, entra, not_found, page, urls};

    const SAM: &str = "00000000-0000-4000-8000-00000000e102";

    #[test]
    fn a_mail_address_finds_the_user_with_their_manager_groups_and_roles() {
        let (outcome, transport) = entra(
            &["entra", "user", "get", "sam@contoso.com"],
            vec![
                not_found(),
                page(vec![
                    json!({"id": SAM, "displayName": "Sam Lee", "userPrincipalName": "sam@contoso.example",
                    "mail": "sam@contoso.com", "userType": "Member", "accountEnabled": true,
                    "createdDateTime": "2024-02-01T09:00:00Z", "onPremisesSyncEnabled": null,
                    "department": "SRE", "jobTitle": "Site reliability engineer"}),
                ]),
                not_found(),
                page(vec![
                    json!({"@odata.type": "#microsoft.graph.group", "id": "g3", "displayName": "sg-contoso-oncall"}),
                    json!({"@odata.type": "#microsoft.graph.directoryRole", "id": "r1", "displayName": "Reports Reader"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": SAM, "name": "Sam Lee", "upn": "sam@contoso.example", "mail": "sam@contoso.com",
                "type": "Member", "enabled": true, "created": "2024-02-01T09:00:00Z", "department": "SRE",
                "job_title": "Site reliability engineer",
                "groups": [{"id": "g3", "name": "sg-contoso-oncall"}], "roles": ["Reports Reader"]})
        );
        let sent = urls(&transport);
        assert!(
            sent[0].starts_with(&format!("{V1}/users/sam%40contoso.com?$select=id,")),
            "{sent:?}"
        );
        assert!(
            sent[1].starts_with(&format!(
                "{V1}/users?$filter=mail%20eq%20%27sam%40contoso.com%27&$select="
            )),
            "{sent:?}"
        );
        assert_eq!(
            sent[3],
            format!("{V1}/users/{SAM}/transitiveMemberOf?$select=id,displayName")
        );
    }
}
