use agent_cli_core::{Ctx, Failure, command, scoped};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{At, ControlM};

use super::DefRow;

#[derive(clap::Args)]
pub struct DefinitionGetArgs {
    /// The definition: SERVER/FOLDER/JOB, as definition list and job list
    /// print it, or INSTANCE:SERVER/FOLDER/JOB as definition diff does
    id: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DefDetail {
    #[serde(flatten)]
    row: DefRow,
    /// The whole definition as Control-M holds it: its schedule (When), the
    /// conditions it waits for and adds (eventsToWaitFor, eventsToAdd),
    /// variables and notifications.
    definition: Value,
}

// ponytail: `definition` is Control-M's JSON as it comes; name its schedule
// and conditions as fields (the jobs it waits for are what agents trace)
// once real definitions show which keys matter.
fn definition_get(ctx: &Ctx, args: DefinitionGetArgs) -> Result<DefDetail> {
    let controlm = ControlM::load(ctx.config())?;
    let (mut at, mut id) = (args.at.clone(), args.id.as_str());
    if let (Some(scope), rest) = scoped(&args.id)
        && controlm
            .instances
            .iter()
            .any(|instance| instance.name == scope)
    {
        if let Some(flag) = at.instance.as_deref().filter(|flag| *flag != scope) {
            return Err(Failure::usage(format!(
                "{} names instance {scope}, and --instance says {flag}",
                args.id
            ))
            .into());
        }
        (at.instance, id) = (Some(scope.to_owned()), rest);
    }
    let client = controlm.open(ctx, &at)?;
    let found = super::one(&client, id)?;
    Ok(DefDetail {
        row: found.row,
        definition: found.definition,
    })
}

command! {
    pub DEFINITION_GET = ["controlm", "definition", "get"], Read,
    "Show a Control-M job's definition: its command, schedule and conditions",
    keywords: ["control-m", "ctm", "batch", "schedule", "calendar", "when", "conditions", "depends", "upstream", "command"],
    example: "controlm definition get ctm-prod/NightlyLoads/load_orders",
    run: definition_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{controlm, paths};

    fn folder() -> serde_json::Value {
        json!({"NightlyLoads": {
            "Type": "Folder", "ControlmServer": "ctm-prod",
            "load_orders": {"Type": "Job:Command", "Command": "/opt/etl/load_orders.sh",
                "When": {"WeekDays": ["MON", "TUE", "WED", "THU", "FRI"], "FromTime": "0015"}}
        }})
    }

    #[test]
    fn definition_get_prints_the_whole_definition() {
        let (outcome, transport) = controlm(
            &[
                "controlm",
                "definition",
                "get",
                "ctm-prod/NightlyLoads/load_orders",
            ],
            vec![Answer::json(&folder())],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            paths(&transport),
            ["deploy/jobs?format=json&ctm=ctm-prod&folder=NightlyLoads&job=load_orders"]
        );
        let got = outcome.json();
        assert_eq!(got["id"], "ctm-prod/NightlyLoads/load_orders");
        assert_eq!(got["definition"]["When"]["FromTime"], "0015");
    }

    #[test]
    fn a_side_as_definition_diff_prints_it_names_its_instance() {
        let (outcome, _) = controlm(
            &[
                "controlm",
                "definition",
                "get",
                "prod:ctm-prod/NightlyLoads/load_orders",
            ],
            vec![Answer::json(&folder())],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["id"], "ctm-prod/NightlyLoads/load_orders");
        let (outcome, _) = controlm(
            &[
                "controlm",
                "definition",
                "get",
                "prod:ctm-prod/NightlyLoads/load_orders",
                "--instance",
                "qa",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_definition_not_in_the_answer_is_exit_4_naming_the_search() {
        let (outcome, _) = controlm(
            &[
                "controlm",
                "definition",
                "get",
                "ctm-prod/NightlyLoads/load_order",
            ],
            vec![Answer::json(&folder())],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli controlm definition list --name load_order")
        );
    }
}
