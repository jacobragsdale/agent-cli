//! `entra principal diff`: two users, groups or service principals side by
//! side, for "Alice can, Bob can't": the groups each is in (nested ones
//! too, from `transitiveMemberOf`), the directory roles that holds, and the
//! Azure role assignments that apply to each, held directly or through one
//! of those groups (Resource Graph, as `arm role list` reads them). Each set
//! is keyed by name, so one group more is one change.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Side, command, diff};
use anyhow::Result;
use serde_json::{Value, json};

use crate::arm::role::assignments;
use crate::client::text;
use crate::config::Azure;
use crate::entra::{Graph, Kind, V1, find};

/// Two principals are no environments: their names are never lined up.
const RULES: Rules = Rules {
    infer: false,
    ..Rules::PLAIN
};

#[derive(clap::Args)]
pub struct PrincipalDiffArgs {
    /// One user, group or service principal: an object id, app id, UPN, name or @me
    left: String,
    /// The other, in the same forms
    right: String,
    #[command(flatten)]
    diff: DiffArgs,
}

/// One principal as compared: its kind, and each set as name → how it is
/// held (`direct`, `nested`, or the group an Azure role comes through).
fn side(ctx: &Ctx, graph: &Graph<'_>, azure: &Azure, raw: &str) -> Result<Side> {
    let (kind, held) = find(
        graph,
        raw,
        &[Kind::User, Kind::ServicePrincipal, Kind::Group],
        "",
    )?;
    let id = text(&held["id"]).unwrap_or_default();
    let at = format!("{V1}/{}/{id}", kind.collection());
    let direct: Vec<String> = graph
        .pages(&format!("{at}/memberOf?$select=id"), usize::MAX)?
        .rows
        .iter()
        .filter_map(|object| text(&object["id"]))
        .collect();
    let held_by = |object: &Value| {
        let direct = text(&object["id"]).is_some_and(|id| direct.contains(&id));
        json!(if direct { "direct" } else { "nested" })
    };
    let mut groups = BTreeMap::new();
    let mut roles = BTreeMap::new();
    let mut group_names = BTreeMap::new();
    // VERIFY(work): that transitiveMemberOf lists the activated directory
    // roles a principal holds, through role-assignable groups too.
    for object in graph
        .pages(
            &format!("{at}/transitiveMemberOf?$select=id,displayName"),
            usize::MAX,
        )?
        .rows
    {
        let (Some(object_id), Some(name)) = (text(&object["id"]), text(&object["displayName"]))
        else {
            continue;
        };
        match object["@odata.type"].as_str() {
            Some("#microsoft.graph.group") => {
                groups.insert(name.clone(), held_by(&object));
                group_names.insert(object_id, name);
            }
            Some("#microsoft.graph.directoryRole") => {
                roles.insert(name, held_by(&object));
            }
            _ => {}
        }
    }
    let mut ids = vec![id.as_str()];
    ids.extend(group_names.keys().map(String::as_str));
    let mut azure_roles = BTreeMap::new();
    for assignment in assignments(ctx, azure, &ids, None, usize::MAX)? {
        let role = assignment.role.unwrap_or(assignment.role_id);
        let via = group_names
            .get(&assignment.principal_id)
            .map_or_else(|| "direct".to_owned(), |group| format!("via {group}"));
        azure_roles.insert(format!("{role} at {}", assignment.scope), json!(via));
    }
    let name = text(&held["userPrincipalName"])
        .or_else(|| text(&held["displayName"]))
        .unwrap_or_else(|| id.clone());
    Ok(Side::new(
        name,
        json!({
            "kind": kind.word(),
            "groups": groups,
            "directory_roles": roles,
            "azure_roles": azure_roles,
        }),
    ))
}

fn principal_diff(ctx: &Ctx, args: PrincipalDiffArgs) -> Result<Diff> {
    let graph = Graph::new(ctx);
    let azure = Azure::load(ctx)?;
    let left = side(ctx, &graph, &azure, &args.left)?;
    let right = side(ctx, &graph, &azure, &args.right)?;
    diff(ctx, &args.diff, &RULES, left, right)
}

command! {
    pub PRINCIPAL_DIFF = ["entra", "principal", "diff"], Read,
    "Compare two users or identities: groups (nested too), directory and Azure roles",
    keywords: ["access", "can", "cannot", "permission", "membership", "groups", "rbac", "why"],
    example: "entra principal diff alice@contoso.example bob@contoso.example --fields same,changes",
    run: principal_diff,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::entra::testing::{V1, entra, page, urls};
    use crate::testing;

    const ALICE: &str = "00000000-0000-4000-8000-00000000a11c";
    const BOB: &str = "00000000-0000-4000-8000-0000000000b0";
    const PLATFORM: &str = "00000000-0000-4000-8000-00000000f001";
    const READERS: &str = "00000000-0000-4000-8000-00000000f002";

    fn user(id: &str, upn: &str) -> serde_json::Value {
        json!({"@odata.type": "#microsoft.graph.user", "id": id, "displayName": upn, "userPrincipalName": upn})
    }

    fn group(id: &str, name: &str) -> serde_json::Value {
        json!({"@odata.type": "#microsoft.graph.group", "id": id, "displayName": name})
    }

    fn reader(principal: &str) -> serde_json::Value {
        json!({"id": "a1", "principalId": principal, "principalType": "Group", "role": "Key Vault Secrets User",
            "roleId": "4633458b-17de-408a-b874-0445c86b69e6", "scope": format!("{}/providers/Microsoft.KeyVault/vaults/kv-contoso-prod", testing::RG)})
    }

    #[test]
    fn alice_reads_the_vault_through_a_nested_group_bob_is_not_in() {
        let (outcome, transport) = entra(
            &["entra", "principal", "diff", ALICE, BOB],
            vec![
                page(vec![user(ALICE, "alice@contoso.example")]),
                page(vec![group(PLATFORM, "sg-contoso-platform")]),
                page(vec![
                    group(PLATFORM, "sg-contoso-platform"),
                    group(READERS, "sg-contoso-vault-readers"),
                ]),
                testing::inventory(vec![reader(READERS)]),
                page(vec![user(BOB, "bob@contoso.example")]),
                page(vec![group(PLATFORM, "sg-contoso-platform")]),
                page(vec![group(PLATFORM, "sg-contoso-platform")]),
                testing::inventory(vec![]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["left"], "alice@contoso.example");
        assert_eq!(got["same"], false);
        let paths: Vec<&str> = got["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| change["path"].as_str().unwrap())
            .collect();
        assert!(paths.contains(&"groups.sg-contoso-vault-readers"), "{got}");
        // Bob holds no Azure role at all, so the whole set is the change.
        let roles = got["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|change| change["path"] == "azure_roles")
            .unwrap();
        assert!(roles.get("right").is_none(), "{roles}");
        assert!(
            roles["left"]
                .to_string()
                .contains("Key Vault Secrets User at")
                && roles["left"]
                    .to_string()
                    .contains("via sg-contoso-vault-readers"),
            "{roles}"
        );
        assert!(
            got.get("as").is_none(),
            "two people are no environments: {got}"
        );
        let sent = urls(&transport);
        assert!(
            sent.contains(&format!(
                "{V1}/users/{ALICE}/transitiveMemberOf?$select=id,displayName"
            )),
            "{sent:?}"
        );
        let query = transport.sent()[3].body.clone().unwrap()["query"].to_string();
        assert!(query.contains(ALICE) && query.contains(READERS), "{query}");
    }
}
