//! `entra member list`: who is in a group. `/members` is direct only and
//! `/transitiveMembers` cannot say through which group, so the walk reads
//! `/members` of the group and then of each nested group, a level at a
//! time on `[azure] parallel` threads, each group once (cycles end there),
//! within the deadline. `via` is the chain of group names down from the
//! group asked about.

use std::time::Duration;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{guid, parallel, text};
use crate::config::Azure;
use crate::entra::{Graph, Kind, V1, find};

const SELECT: &str = "id,displayName,userPrincipalName,appId,servicePrincipalType,accountEnabled";
/// What the walk leaves of the deadline for printing.
const MARGIN: Duration = Duration::from_secs(5);

#[derive(clap::Args)]
pub struct MemberListArgs {
    /// The group: object id (arm role list prints it as principal_id), display name, or portal link
    group: String,
    /// Only its direct members: do not walk into nested groups
    #[arg(long)]
    direct: bool,
    /// Only members of this type
    #[arg(long = "type", value_parser = PossibleValuesParser::new(["user", "group", "serviceprincipal"]))]
    kind: Option<String>,
    /// Only how this user, group or service principal is in it (every path): an id, UPN, name or @me
    #[arg(long)]
    principal: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MemberRow {
    /// The object id, which the get of its type takes.
    id: String,
    /// user, group or serviceprincipal.
    #[serde(rename = "type")]
    kind: String,
    name: Option<String>,
    upn: Option<String>,
    /// A service principal's client id.
    app_id: Option<String>,
    enabled: Option<bool>,
    /// The nested groups it is in it through, outermost first; empty when direct.
    via: Vec<String>,
}

fn row(member: &Value, via: &[String]) -> Option<MemberRow> {
    Some(MemberRow {
        id: text(&member["id"])?,
        kind: Kind::of(member).map_or("other", Kind::word).to_owned(),
        name: text(&member["displayName"]),
        upn: text(&member["userPrincipalName"]),
        app_id: text(&member["appId"]),
        enabled: member["accountEnabled"].as_bool(),
        via: via.to_vec(),
    })
}

fn member_list(ctx: &Ctx, args: MemberListArgs) -> Result<Vec<MemberRow>> {
    let graph = Graph::new(ctx);
    let (_, group) = find(
        &graph,
        &args.group,
        &[Kind::Group],
        "id,displayName,membershipRule,membershipRuleProcessingState",
    )?;
    let root = text(&group["id"]).unwrap_or_default();
    let principal = match args.principal.as_deref().map(str::trim) {
        None => None,
        Some(raw) if guid(raw) => Some(raw.to_ascii_lowercase()),
        Some(raw) => {
            let (_, held) = find(
                &graph,
                raw,
                &[Kind::User, Kind::ServicePrincipal, Kind::Group],
                "",
            )?;
            text(&held["id"]).map(|id| id.to_ascii_lowercase())
        }
    };
    let threads = Azure::load(ctx)?.parallel();
    let mut seen = vec![root.clone()];
    let mut level: Vec<(String, Vec<String>)> = vec![(root.clone(), Vec::new())];
    let mut rows: Vec<MemberRow> = Vec::new();
    let mut nested = 0;
    let mut complete = true;
    while !level.is_empty() {
        if level[0].0 != root && ctx.remaining()? < MARGIN {
            complete = false;
            break;
        }
        let pages = parallel(&level, threads, |(id, _)| {
            graph.pages(
                &format!("{V1}/groups/{id}/members?$select={SELECT}&$top=999"),
                usize::MAX,
            )
        });
        let mut next = Vec::new();
        for ((_, via), page) in level.iter().zip(pages) {
            for member in &page?.rows {
                let Some(found) = row(member, via) else {
                    continue;
                };
                if found.kind == "group" && !args.direct && !seen.contains(&found.id) {
                    seen.push(found.id.clone());
                    nested += 1;
                    let mut deeper = via.clone();
                    deeper.push(found.name.clone().unwrap_or_else(|| found.id.clone()));
                    next.push((found.id.clone(), deeper));
                }
                rows.push(found);
            }
        }
        level = next;
    }
    let name = text(&group["displayName"]).unwrap_or_else(|| root.clone());
    if !complete {
        ctx.note(format!(
            "[the walk stopped at the deadline; members of deeper groups are missing: agent-cli entra member list {root} --direct, or a larger --timeout]"
        ));
    }
    if let Some(rule) = text(&group["membershipRule"]) {
        let state = text(&group["membershipRuleProcessingState"]).unwrap_or_default();
        ctx.note(format!("[dynamic ({state}): {rule}]"));
    }
    if let Some(principal) = principal {
        rows.retain(|held| held.id.eq_ignore_ascii_case(&principal));
        if rows.is_empty() {
            ctx.note(format!("[{principal} is not in {name}]"));
        }
        return Ok(rows);
    }
    // The first path found is the shortest: the walk goes a level at a time.
    let mut unique: Vec<MemberRow> = Vec::new();
    for held in rows {
        if !unique.iter().any(|kept| kept.id == held.id) {
            unique.push(held);
        }
    }
    if let Some(kind) = &args.kind {
        unique.retain(|held| held.kind == *kind);
    }
    let total = unique.len();
    let through = unique.iter().filter(|held| !held.via.is_empty()).count();
    if total > args.limit {
        unique.truncate(args.limit);
        let nested = if through > 0 {
            format!(", {through} through {nested} nested groups")
        } else {
            String::new()
        };
        ctx.note(format!(
            "[{} of {total} members{nested}; --limit N]",
            args.limit
        ));
    }
    Ok(unique)
}

command! {
    pub MEMBER_LIST = ["entra", "member", "list"], Read,
    "List who is in an Entra group, nested groups included, and through which",
    keywords: ["members", "nested", "transitive", "who", "users", "through", "via", "security"],
    example: "entra member list sg-contoso-platform --fields name,upn,via",
    run: member_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::entra::testing::{V1, entra, page, urls};

    const PLATFORM: &str = "00000000-0000-4000-8000-00000000f001";
    const ONCALL: &str = "00000000-0000-4000-8000-00000000f003";
    const SAM: &str = "00000000-0000-4000-8000-00000000e102";

    fn tree() -> Vec<Answer> {
        vec![
            Answer::json(&json!({"id": PLATFORM, "displayName": "sg-contoso-platform"})),
            page(vec![
                json!({"@odata.type": "#microsoft.graph.user", "id": "u1", "displayName": "Dana", "userPrincipalName": "dana@contoso.example", "accountEnabled": true}),
                json!({"@odata.type": "#microsoft.graph.group", "id": ONCALL, "displayName": "sg-contoso-oncall"}),
            ]),
            page(vec![
                json!({"@odata.type": "#microsoft.graph.user", "id": SAM, "displayName": "Sam Lee", "userPrincipalName": "sam@contoso.example", "accountEnabled": true}),
                json!({"@odata.type": "#microsoft.graph.group", "id": PLATFORM, "displayName": "sg-contoso-platform"}),
            ]),
        ]
    }

    #[test]
    fn nested_members_say_through_which_group_and_a_cycle_ends_the_walk() {
        let (outcome, transport) = entra(
            &["entra", "member", "list", PLATFORM, "--limit", "2"],
            tree(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "u1", "type": "user", "name": "Dana", "upn": "dana@contoso.example", "enabled": true},
                {"id": ONCALL, "type": "group", "name": "sg-contoso-oncall"},
            ])
        );
        assert!(
            outcome
                .stderr
                .contains("[2 of 4 members, 2 through 1 nested groups; --limit N]"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            urls(&transport)[2],
            format!(
                "{V1}/groups/{ONCALL}/members?$select=id,displayName,userPrincipalName,appId,servicePrincipalType,accountEnabled&$top=999"
            )
        );
        assert_eq!(transport.sent().len(), 3);
    }

    #[test]
    fn a_principal_shows_only_its_paths_or_a_note_that_it_is_not_in() {
        let (outcome, _) = entra(
            &["entra", "member", "list", PLATFORM, "--principal", SAM],
            tree(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["via"], json!(["sg-contoso-oncall"]));
        let (outcome, _) = entra(
            &[
                "entra",
                "member",
                "list",
                PLATFORM,
                "--principal",
                "00000000-0000-4000-8000-0000000000ff",
            ],
            tree(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("[00000000-0000-4000-8000-0000000000ff is not in sg-contoso-platform]"),
            "{}",
            outcome.stderr
        );
    }
}
