use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, list, text};

#[derive(clap::Args)]
pub struct WorkitemTypeListArgs {
    /// The project (default: the first in [ado] project)
    #[arg(long)]
    project: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkitemTypeRow {
    /// What workitem-type get, and workitem create --type, take.
    name: String,
    description: Option<String>,
    states: Vec<String>,
}

fn workitem_type_list(ctx: &Ctx, args: WorkitemTypeListArgs) -> Result<Vec<WorkitemTypeRow>> {
    let ado = Ado::load_in(ctx, args.project.as_deref())?;
    let answer = ado.get(ctx, &ado.work("wit/workitemtypes", ""))?;
    Ok(list(&answer["value"])
        .iter()
        // A disabled type can be read but no longer created.
        .filter(|kind| kind["isDisabled"] != true)
        .filter_map(|kind| {
            Some(WorkitemTypeRow {
                name: text(&kind["name"])?,
                description: text(&kind["description"]),
                states: list(&kind["states"])
                    .iter()
                    .filter_map(|state| text(&state["name"]))
                    .collect(),
            })
        })
        .take(args.limit)
        .collect())
}

command! {
    pub WORKITEM_TYPE_LIST = ["ado", "workitem-type", "list"], Read,
    "List the project's work item types (Bug, User Story, Task …) and their states",
    keywords: ["kinds", "process", "workflow", "available", "allowed"],
    example: "ado workitem-type list --fields name,states",
    run: workitem_type_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{CODE, ado, page, urls};

    #[test]
    fn types_list_with_their_states_and_without_the_disabled_ones() {
        let (outcome, transport) = ado(
            &["ado", "workitem-type", "list"],
            vec![page(vec![
                json!({"name": "Bug", "description": "A defect", "isDisabled": false,
                    "xmlForm": "<FORM/>", "states": [
                        {"name": "New", "color": "b2b2b2", "category": "Proposed"},
                        {"name": "Closed", "color": "339933", "category": "Completed"}]}),
                json!({"name": "Impediment", "isDisabled": true, "states": []}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "Bug", "description": "A defect", "states": ["New", "Closed"]}])
        );
        assert_eq!(
            urls(&transport),
            [format!("{CODE}/wit/workitemtypes?api-version=7.1")]
        );
    }
}
