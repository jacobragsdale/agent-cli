//! `arm change list`: Resource Graph's `resourcechanges`, one query across
//! the subscriptions in scope, each change with the properties it moved.

use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::arm::quote;
use crate::arm::resource::any;
use crate::client::{stamp, text};
use crate::config::Azure;
use crate::graph::rows;

/// What one change lists of the properties it moved.
const PROPERTIES: usize = 20;

#[derive(clap::Args)]
pub struct ChangeListArgs {
    /// Only changes to this resource and what sits under it (its role assignments too): an ARM id, portal link or name
    #[arg(long)]
    resource: Option<String>,
    /// Only changes in this resource group
    #[arg(long)]
    group: Option<String>,
    /// From when (default 14 days ago, all Resource Graph keeps)
    #[arg(long)]
    since: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChangeRow {
    when: Option<String>,
    /// The ARM id that changed, which `arm resource get` takes.
    resource: String,
    /// Create, Update or Delete.
    change_type: Option<String>,
    /// Who, when Azure says.
    changed_by: Option<String>,
    properties: Vec<Moved>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Moved {
    /// `properties.count`, `tags.owner`.
    path: String,
    before: Option<Value>,
    after: Option<Value>,
}

fn kql(target: Option<&str>, group: Option<&str>, since: When) -> String {
    let mut kql = format!(
        "resourcechanges\
         \n| extend changeTime = todatetime(properties.changeAttributes.timestamp), target = tostring(properties.targetResourceId)\
         \n| where changeTime >= datetime({})",
        since.utc()
    );
    if let Some(id) = target {
        kql.push_str(&format!(
            "\n| where target =~ {} or target startswith {}",
            quote(id),
            quote(&format!("{id}/"))
        ));
    }
    // VERIFY(work): that resourcechanges rows carry resourceGroup.
    if let Some(group) = group {
        kql.push_str(&format!("\n| where resourceGroup =~ {}", quote(group)));
    }
    kql.push_str(
        "\n| project id, changeTime, target, changeType = tostring(properties.changeType), \
         changedBy = tostring(properties.changeAttributes.changedBy), changes = properties.changes\
         \n| order by changeTime desc, id asc",
    );
    kql
}

fn change_list(ctx: &Ctx, args: ChangeListArgs) -> Result<Vec<ChangeRow>> {
    let azure = Azure::load(ctx)?;
    let target = match &args.resource {
        Some(raw) => Some(any(ctx, &azure, raw, args.group.as_deref())?),
        None => None,
    };
    let since = args
        .since
        .unwrap_or(When(agent_cli_core::now() - time::Duration::days(14)));
    let group = if target.is_some() {
        None
    } else {
        args.group.as_deref()
    };
    let kql = kql(target.as_ref().map(|t| t.id.as_str()), group, since);
    let mut cut = false;
    let found = rows(ctx, &azure.subscriptions, &kql, args.limit)?
        .iter()
        .map(|row| {
            let mut properties: Vec<Moved> = row["changes"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(path, moved)| Moved {
                    path: path.clone(),
                    before: moved.get("previousValue").filter(|v| !v.is_null()).cloned(),
                    after: moved.get("newValue").filter(|v| !v.is_null()).cloned(),
                })
                .collect();
            if properties.len() > PROPERTIES {
                cut = true;
                properties.truncate(PROPERTIES);
            }
            ChangeRow {
                when: stamp(&row["changeTime"]),
                resource: text(&row["target"]).unwrap_or_default(),
                change_type: text(&row["changeType"]),
                changed_by: text(&row["changedBy"]),
                properties,
            }
        })
        .collect();
    if cut {
        ctx.note(format!(
            "[a change lists its first {PROPERTIES} properties; arm resource get shows the rest as they are now]"
        ));
    }
    Ok(found)
}

command! {
    pub CHANGE_LIST = ["arm", "change", "list"], Read,
    "List what changed on Azure resources: who, when, each property before and after",
    keywords: ["changed", "history", "diff", "before", "after", "modified", "deleted", "drift"],
    example: "arm change list --resource kv-contoso-prod --since 2d --fields when,resource,change_type,changed_by",
    run: change_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{azure, inventory};

    const VAULT: &str = "/subscriptions/00000000-0000-4000-8000-000000000001/resourceGroups/rg-contoso/providers/Microsoft.KeyVault/vaults/kv-contoso";

    #[test]
    fn a_resources_changes_include_what_sits_under_it_with_each_property_moved() {
        let (outcome, transport) = azure(
            &[crate::ARM],
            &[
                "arm",
                "change",
                "list",
                "--resource",
                VAULT,
                "--since",
                "2026-09-28",
            ],
            vec![inventory(vec![
                json!({"id": "c1", "changeTime": "2026-09-28T16:42:10.123Z",
                    "target": format!("{VAULT}/providers/Microsoft.Authorization/roleAssignments/r1"),
                    "changeType": "Delete", "changedBy": "dana@contoso.example", "changes": {}}),
                json!({"id": "c2", "changeTime": "2026-09-28T08:00:00Z", "target": VAULT,
                    "changeType": "Update", "changedBy": "",
                    "changes": {"tags.owner": {"previousValue": null, "newValue": "platform", "propertyChangeType": "Insert"}}}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"when": "2026-09-28T16:42:10Z", "resource": format!("{VAULT}/providers/Microsoft.Authorization/roleAssignments/r1"),
                    "change_type": "Delete", "changed_by": "dana@contoso.example"},
                {"when": "2026-09-28T08:00:00Z", "resource": VAULT, "change_type": "Update",
                    "properties": [{"path": "tags.owner", "after": "platform"}]},
            ])
        );
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert_eq!(
            query,
            format!(
                "resourcechanges\n| extend changeTime = todatetime(properties.changeAttributes.timestamp), target = tostring(properties.targetResourceId)\n| where changeTime >= datetime(2026-09-28T00:00:00Z)\n| where target =~ '{VAULT}' or target startswith '{VAULT}/'\n| project id, changeTime, target, changeType = tostring(properties.changeType), changedBy = tostring(properties.changeAttributes.changedBy), changes = properties.changes\n| order by changeTime desc, id asc"
            )
        );
    }
}
