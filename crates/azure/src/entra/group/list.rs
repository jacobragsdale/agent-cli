//! `entra group list`: groups by name, the groups a principal is in (nested
//! ones too, `direct` saying which it was added to), or those a user owns.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{limited, text};
use crate::entra::group::kind;
use crate::entra::{Graph, Kind, V1, find, odata, param};

const SELECT: &str = "id,displayName,groupTypes,securityEnabled,mailEnabled";

#[derive(clap::Args)]
pub struct GroupListArgs {
    /// Only groups whose name starts with this
    text: Option<String>,
    /// Only the groups this user, group or service principal is in, nested ones too: an id, UPN, name or @me
    #[arg(long, conflicts_with = "owner")]
    member: Option<String>,
    /// Only the groups this user or service principal owns: an id, UPN, name, or @me (the az login)
    #[arg(long)]
    owner: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GroupRow {
    /// What group get and member list take.
    id: String,
    name: Option<String>,
    /// security, m365, mail-security or distribution.
    kind: String,
    /// Members come from a rule, not by hand.
    dynamic: bool,
    /// With --member: added to it directly, not through a nested group.
    direct: Option<bool>,
}

fn row(group: &Value, direct: Option<bool>) -> Option<GroupRow> {
    let (kind, dynamic) = kind(group);
    Some(GroupRow {
        id: text(&group["id"])?,
        name: text(&group["displayName"]),
        kind,
        dynamic,
        direct,
    })
}

fn group_list(ctx: &Ctx, args: GroupListArgs) -> Result<Vec<GroupRow>> {
    let graph = Graph::new(ctx);
    let starts = |group: &GroupRow| {
        args.text.as_deref().is_none_or(|prefix| {
            group.name.as_deref().is_some_and(|name| {
                name.to_lowercase()
                    .starts_with(&prefix.trim().to_lowercase())
            })
        })
    };
    if let Some(member) = &args.member {
        let (kind, held) = find(
            &graph,
            member,
            &[Kind::User, Kind::ServicePrincipal, Kind::Group],
            "",
        )?;
        let at = format!(
            "{V1}/{}/{}",
            kind.collection(),
            text(&held["id"]).unwrap_or_default()
        );
        let direct: Vec<String> = graph
            .pages(&format!("{at}/memberOf?$select=id"), usize::MAX)?
            .rows
            .iter()
            .filter_map(|group| text(&group["id"]))
            .collect();
        let all = graph.pages(
            &format!("{at}/transitiveMemberOf?$select={SELECT}"),
            usize::MAX,
        )?;
        let rows: Vec<GroupRow> = all
            .rows
            .iter()
            .filter(|group| Kind::of(group) == Some(Kind::Group))
            .filter_map(|group| {
                let id = text(&group["id"])?;
                row(group, Some(direct.contains(&id)))
            })
            .filter(starts)
            .collect();
        return Ok(limited(ctx, rows, args.limit));
    }
    if let Some(owner) = &args.owner {
        let (kind, held) = find(&graph, owner, &[Kind::User, Kind::ServicePrincipal], "")?;
        // VERIFY(work): that ownedObjects takes group properties in $select.
        let owned = graph.pages(
            &format!(
                "{V1}/{}/{}/ownedObjects?$select={SELECT}",
                kind.collection(),
                text(&held["id"]).unwrap_or_default()
            ),
            usize::MAX,
        )?;
        let rows: Vec<GroupRow> = owned
            .rows
            .iter()
            .filter(|group| Kind::of(group) == Some(Kind::Group))
            .filter_map(|group| row(group, None))
            .filter(starts)
            .collect();
        return Ok(limited(ctx, rows, args.limit));
    }
    let mut url = format!(
        "{V1}/groups?$select={SELECT}&$top={}",
        (args.limit + 1).min(999)
    );
    if let Some(prefix) = &args.text {
        url.push_str(&format!(
            "&$filter={}",
            param(&format!("startswith(displayName,{})", odata(prefix.trim())))
        ));
    }
    let found = graph.pages(&url, args.limit)?;
    if found.more {
        ctx.note(format!(
            "[first {}; --limit N, or narrow by the name's start]",
            args.limit
        ));
    }
    Ok(found
        .rows
        .iter()
        .filter_map(|group| row(group, None))
        .collect())
}

command! {
    pub GROUP_LIST = ["entra", "group", "list"], Read,
    "List Entra groups by name, or the groups a user or service principal is in",
    keywords: ["security", "m365", "memberof", "belongs", "nested", "transitive", "dynamic", "owned", "starting", "prefix"],
    example: "entra group list --member sam@contoso.example --fields id,name,direct",
    run: group_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::entra::testing::{V1, entra, page, urls};

    const SAM: &str = "00000000-0000-4000-8000-00000000e102";

    #[test]
    fn a_members_groups_say_which_were_direct_and_a_name_lists_by_its_start() {
        let (outcome, transport) = entra(
            &["entra", "group", "list", "--member", SAM],
            vec![
                page(vec![
                    json!({"@odata.type": "#microsoft.graph.user", "id": SAM}),
                ]),
                page(vec![json!({"id": "g3"})]),
                page(vec![
                    json!({"@odata.type": "#microsoft.graph.group", "id": "g3", "displayName": "sg-contoso-oncall",
                        "groupTypes": [], "securityEnabled": true, "mailEnabled": false}),
                    json!({"@odata.type": "#microsoft.graph.group", "id": "g1", "displayName": "sg-contoso-platform",
                        "groupTypes": [], "securityEnabled": true, "mailEnabled": false}),
                    json!({"@odata.type": "#microsoft.graph.directoryRole", "id": "r1", "displayName": "Reports Reader"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "g3", "name": "sg-contoso-oncall", "kind": "security", "dynamic": false, "direct": true},
                {"id": "g1", "name": "sg-contoso-platform", "kind": "security", "dynamic": false, "direct": false},
            ])
        );
        assert_eq!(
            urls(&transport)[2],
            format!(
                "{V1}/users/{SAM}/transitiveMemberOf?$select=id,displayName,groupTypes,securityEnabled,mailEnabled"
            )
        );

        let (outcome, transport) = entra(
            &["entra", "group", "list", "sg-contoso", "--limit", "1"],
            vec![page(vec![
                json!({"id": "g4", "displayName": "sg-contoso-sre", "groupTypes": ["DynamicMembership"],
                    "securityEnabled": true, "mailEnabled": false}),
                json!({"id": "g5", "displayName": "sg-contoso-all", "groupTypes": ["Unified"],
                    "securityEnabled": false, "mailEnabled": true}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "g4", "name": "sg-contoso-sre", "kind": "security", "dynamic": true}])
        );
        assert!(outcome.stderr.contains("[first 1;"), "{}", outcome.stderr);
        assert_eq!(
            urls(&transport)[0],
            format!(
                "{V1}/groups?$select=id,displayName,groupTypes,securityEnabled,mailEnabled&$top=2&$filter=startswith%28displayName%2C%27sg-contoso%27%29"
            )
        );
    }
}
