//! `ado variable-group diff`: two pipeline variable groups (staging's and
//! prod's) by variable: those on one side only and each value; a secret
//! variable's value Azure DevOps never returns, so its equality is unknown.
//! The groups are the pipelines' project's (`distributedtask/variablegroups`).

use agent_cli_core::{Ctx, Diff, DiffArgs, Failure, Rules, Side, command, diff, percent_encode};
use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::client::{Ado, list, text};

#[derive(clap::Args)]
pub struct VariableGroupDiffArgs {
    /// One group: its name or id
    left: String,
    /// The other group, as left
    right: String,
    #[command(flatten)]
    diff: DiffArgs,
}

fn variable_group_diff(ctx: &Ctx, args: VariableGroupDiffArgs) -> Result<Diff> {
    let ado = Ado::load(ctx)?;
    let side = |raw: &str| -> Result<Side> {
        let group = group(ctx, &ado, raw.trim())?;
        let mut variables = Map::new();
        let mut side_secrets = std::collections::BTreeMap::new();
        for (key, variable) in group["variables"].as_object().into_iter().flatten() {
            if variable["isSecret"] == true {
                side_secrets.insert(format!("variables.{key}"), None);
            } else {
                variables.insert(key.clone(), variable["value"].clone());
            }
        }
        let row = json!({
            "description": group["description"],
            "type": group["type"],
            // A group linked to Key Vault names the vault its secrets come from.
            "key_vault": group["providerData"]["vault"],
            "variables": variables,
        });
        let name = text(&group["name"]).unwrap_or_else(|| raw.to_owned());
        let mut side = Side::new(name, row);
        side.secrets = side_secrets;
        Ok(side)
    };
    let (left, right) = (side(&args.left)?, side(&args.right)?);
    diff(ctx, &args.diff, &Rules::PLAIN, left, right)
}

/// The group a name or id names.
fn group(ctx: &Ctx, ado: &Ado, raw: &str) -> Result<Value> {
    if raw.chars().all(|c| c.is_ascii_digit()) && !raw.is_empty() {
        return ado.get(
            ctx,
            &ado.code(&format!("distributedtask/variablegroups/{raw}"), ""),
        );
    }
    let mut name = String::new();
    percent_encode(raw, &mut name);
    let found = ado.get(
        ctx,
        &ado.code(
            "distributedtask/variablegroups",
            &format!("groupName={name}"),
        ),
    )?;
    list(&found["value"])
        .iter()
        .find(|group| {
            group["name"]
                .as_str()
                .is_some_and(|held| held.eq_ignore_ascii_case(raw))
        })
        .cloned()
        .ok_or_else(|| {
            Failure::not_found(format!("no variable group {raw} in {}", ado.code_project)).into()
        })
}

command! {
    pub VARIABLE_GROUP_DIFF = ["ado", "variable-group", "diff"], Read,
    "Compare two Azure Pipelines library groups: entries one lacks, and each value",
    keywords: ["library", "pipeline", "staging", "settings"],
    example: "ado variable-group diff orders-staging orders-prod",
    run: variable_group_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, urls};

    fn group(name: &str, host: &str, extra: bool) -> Answer {
        let mut variables = json!({
            "DB_HOST": {"value": host},
            "DB_PASSWORD": {"value": null, "isSecret": true},
        });
        if extra {
            variables["FEATURE_FLAGS"] = json!({"value": "retry=on"});
        }
        Answer::json(
            &json!({"count": 1, "value": [{"id": 7, "name": name, "type": "Vsts",
            "description": "orders settings", "variables": variables}]}),
        )
    }

    #[test]
    fn two_groups_differ_by_a_variable_and_secrets_stay_unknown() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "variable-group",
                "diff",
                "orders-staging",
                "orders-prod",
            ],
            vec![
                group("orders-staging", "db-staging.contoso.example", true),
                group("orders-prod", "db-prod.contoso.example", false),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["as"], json!(["staging=prod"]));
        assert_eq!(
            got["changes"],
            json!([
                {"path": "variables.FEATURE_FLAGS", "left": "retry=on"},
                {"path": "variables.DB_PASSWORD", "redacted": true},
            ])
        );
        assert_eq!(
            urls(&transport)[0],
            format!(
                "{CODE}/distributedtask/variablegroups?groupName=orders-staging&api-version=7.1"
            )
        );
    }
}
