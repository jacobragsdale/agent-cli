//! `entra group get`: one group, its rule if dynamic, owners, the groups it
//! is in, and how many members it has directly and through nested groups.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{stamp, text};
use crate::entra::group::kind;
use crate::entra::{Graph, Kind, Named, Owner, V1, find, named, owners};

pub(crate) const SELECT: &str = "id,displayName,description,groupTypes,mail,mailEnabled,securityEnabled,membershipRule,membershipRuleProcessingState,isAssignableToRole,onPremisesSyncEnabled,createdDateTime";

#[derive(clap::Args)]
pub struct GroupGetArgs {
    /// The group: object id (arm role list prints it as principal_id), display name, or portal link
    group: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GroupDetail {
    id: String,
    name: Option<String>,
    description: Option<String>,
    /// security, m365, mail-security or distribution.
    kind: String,
    mail: Option<String>,
    /// A dynamic group's membership rule.
    dynamic_rule: Option<String>,
    /// On or Paused: a paused rule adds and removes no one.
    rule_state: Option<String>,
    /// It can hold directory roles (entra role list shows them).
    role_assignable: Option<bool>,
    synced: Option<bool>,
    created: Option<String>,
    owners: Vec<Owner>,
    /// The groups it is in, nested ones too.
    member_of: Vec<Named>,
    /// Direct members.
    members: Option<u64>,
    /// Members through nested groups too.
    transitive_members: Option<u64>,
}

fn group_get(ctx: &Ctx, args: GroupGetArgs) -> Result<GroupDetail> {
    let graph = Graph::new(ctx);
    let (_, group) = find(&graph, &args.group, &[Kind::Group], SELECT)?;
    let id = text(&group["id"]).unwrap_or_default();
    let owners = owners(&graph, Kind::Group, &id)?;
    let member_of = graph.pages(
        &format!("{V1}/groups/{id}/transitiveMemberOf?$select=id,displayName"),
        usize::MAX,
    )?;
    // VERIFY(work): $count answers from an index that can lag recent changes.
    let count = |path: &str| {
        graph
            .count(&format!("{V1}/groups/{id}/{path}/$count"))
            .unwrap_or_else(|error| {
                ctx.note(format!("[{path} not counted: {error:#}]"));
                None
            })
    };
    let (kind, _) = kind(&group);
    let rule_state = text(&group["membershipRuleProcessingState"]);
    if rule_state.as_deref() == Some("Paused") {
        ctx.note(format!(
            "[next: agent-cli entra audit list --target {id} --since 30d]"
        ));
    }
    Ok(GroupDetail {
        name: text(&group["displayName"]),
        description: text(&group["description"]),
        kind,
        mail: text(&group["mail"]),
        dynamic_rule: text(&group["membershipRule"]),
        rule_state,
        role_assignable: group["isAssignableToRole"].as_bool(),
        synced: group["onPremisesSyncEnabled"].as_bool(),
        created: stamp(&group["createdDateTime"]),
        owners,
        member_of: named(&member_of.rows, Kind::Group),
        members: count("members"),
        transitive_members: count("transitiveMembers"),
        id,
    })
}

command! {
    pub GROUP_GET = ["entra", "group", "get"], Read,
    "Show an Entra group: kind, dynamic rule, owners, groups it is in, member counts",
    keywords: ["security", "m365", "dynamic", "rule", "owner", "owners", "nested", "count"],
    example: "entra group get sg-contoso-platform --fields kind,owners,members,transitive_members",
    run: group_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::entra::testing::{entra, page};

    const SRE: &str = "00000000-0000-4000-8000-00000000f004";

    #[test]
    fn a_paused_dynamic_group_shows_its_rule_counts_and_a_note_for_its_audit_log() {
        let (outcome, transport) = entra(
            &["entra", "group", "get", SRE],
            vec![
                Answer::json(
                    &json!({"id": SRE, "displayName": "sg-contoso-sre", "groupTypes": ["DynamicMembership"],
                    "securityEnabled": true, "mailEnabled": false, "membershipRule": "user.department -eq \"SRE\"",
                    "membershipRuleProcessingState": "Paused", "isAssignableToRole": false,
                    "createdDateTime": "2025-03-01T08:00:00Z"}),
                ),
                page(vec![
                    json!({"id": "u1", "displayName": "Dana", "userPrincipalName": "dana@contoso.example", "accountEnabled": true}),
                ]),
                page(vec![]),
                Answer::json(&json!(1)),
                Answer::json(&json!(1)),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            (
                got["kind"].clone(),
                got["dynamic_rule"].clone(),
                got["members"].clone()
            ),
            (
                json!("security"),
                json!("user.department -eq \"SRE\""),
                json!(1)
            )
        );
        assert_eq!(got["owners"][0]["upn"], "dana@contoso.example");
        assert!(
            outcome.stderr.contains(&format!(
                "[next: agent-cli entra audit list --target {SRE} --since 30d]"
            )),
            "{}",
            outcome.stderr
        );
        let sent = transport.sent();
        assert!(sent[3].url.ends_with("/members/$count"), "{}", sent[3].url);
    }
}
