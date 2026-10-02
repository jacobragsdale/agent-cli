use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, list, text};
use crate::types::{self, Field, State};

#[derive(clap::Args)]
pub struct WorkitemTypeGetArgs {
    /// Bug, "User Story", Task … (any case), from workitem-type list
    #[arg(id = "type")]
    kind: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkitemTypeDetail {
    name: String,
    description: Option<String>,
    /// In workflow order; Completed and Removed categories count as done.
    states: Vec<State>,
    /// Each state, and the states it may move to.
    transitions: BTreeMap<String, Vec<String>>,
    /// What --field and the typed flags may set, and what they must be.
    fields: Vec<Field>,
}

fn workitem_type_get(ctx: &Ctx, args: WorkitemTypeGetArgs) -> Result<WorkitemTypeDetail> {
    let ado = Ado::load(ctx)?;
    let kind = types::read_type(ctx, &ado, &args.kind, "", "")?;
    let name = text(&kind["name"]).unwrap_or(args.kind);
    let transitions = kind["transitions"]
        .as_object()
        .into_iter()
        .flatten()
        // `""` is where a new item starts, which states[0] already says.
        .filter(|(from, _)| !from.is_empty())
        .map(|(from, to)| {
            let to = list(to)
                .iter()
                .filter_map(|step| text(&step["to"]))
                .filter(|to| to != from)
                .collect();
            (from.clone(), to)
        })
        .collect();
    Ok(WorkitemTypeDetail {
        states: types::states(ctx, &ado, &name)?,
        fields: types::fields(ctx, &ado, &name)?,
        description: text(&kind["description"]),
        transitions,
        name,
    })
}

command! {
    pub WORKITEM_TYPE_GET = ["ado", "workitem-type", "get"], Read,
    "Show a work item type's states, moves and fields (required, allowed values)",
    keywords: ["rules", "workflow", "move", "valid", "allowed", "picklist", "mandatory", "reference", "custom"],
    example: "ado workitem-type get Bug --fields states,transitions",
    run: workitem_type_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, page, urls};

    #[test]
    fn a_type_shows_its_states_moves_and_fields_and_requires_only_what_nothing_fills() {
        let (outcome, transport) = ado(
            &["ado", "workitem-type", "get", "bug"],
            vec![
                Answer::json(&json!({"name": "Bug", "description": "A defect",
                "transitions": {
                    "": [{"to": "New", "actions": null}],
                    "New": [{"to": "New"}, {"to": "Active", "actions": ["Microsoft.VSTS.Actions.StartWork"]}],
                    "Active": [{"to": "Closed"}, {"to": "New"}]
                }})),
                page(vec![
                    json!({"name": "New", "color": "b2b2b2", "category": "Proposed"}),
                    json!({"name": "Active", "color": "007acc", "category": "InProgress"}),
                    json!({"name": "Closed", "color": "339933", "category": "Completed"}),
                ]),
                page(vec![
                    json!({"name": "Title", "referenceName": "System.Title", "alwaysRequired": true,
                        "defaultValue": null, "allowedValues": []}),
                    json!({"name": "Iteration ID", "referenceName": "System.IterationId",
                        "alwaysRequired": true, "defaultValue": null, "allowedValues": []}),
                    json!({"name": "Priority", "referenceName": "Microsoft.VSTS.Common.Priority",
                        "alwaysRequired": false, "defaultValue": 2, "allowedValues": [1, 2, 3, 4]}),
                    json!({"name": "Severity", "referenceName": "Microsoft.VSTS.Common.Severity",
                        "alwaysRequired": true, "defaultValue": "3 - Medium",
                        "allowedValues": ["1 - Critical", "3 - Medium"]}),
                ]),
                page(vec![
                    json!({"referenceName": "System.Title", "type": "string"}),
                    json!({"referenceName": "Microsoft.VSTS.Common.Priority", "type": "integer"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({
                "name": "Bug",
                "description": "A defect",
                "states": [
                    {"name": "New", "category": "Proposed"},
                    {"name": "Active", "category": "InProgress"},
                    {"name": "Closed", "category": "Completed"}
                ],
                "transitions": {"Active": ["Closed", "New"], "New": ["Active"]},
                "fields": [
                    {"name": "Title", "ref": "System.Title", "type": "string", "required": true},
                    {"name": "Iteration ID", "ref": "System.IterationId", "required": false},
                    {"name": "Priority", "ref": "Microsoft.VSTS.Common.Priority", "type": "integer",
                        "required": false, "allowed_values": ["1", "2", "3", "4"], "default": "2"},
                    {"name": "Severity", "ref": "Microsoft.VSTS.Common.Severity", "required": false,
                        "allowed_values": ["1 - Critical", "3 - Medium"], "default": "3 - Medium"}
                ]
            })
        );
        assert_eq!(
            urls(&transport),
            [
                format!("{CODE}/wit/workitemtypes/bug?api-version=7.1"),
                format!("{CODE}/wit/workitemtypes/Bug/states?api-version=7.1"),
                format!(
                    "{CODE}/wit/workitemtypes/Bug/fields?$expand=allowedValues&api-version=7.1"
                ),
                format!("{CODE}/wit/fields?api-version=7.1"),
            ]
        );
    }

    #[test]
    fn an_unknown_type_is_not_found_and_points_to_the_list() {
        let (outcome, _) = ado(
            &["ado", "workitem-type", "get", "Defect"],
            vec![Answer::status(
                404,
                json!({"message": "VS402323: Work item type Defect does not exist."}).to_string(),
            )],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli ado workitem-type list"),
            "{}",
            outcome.stderr
        );
    }
}
