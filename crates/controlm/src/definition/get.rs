use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{At, ControlM, with_query};

use super::{DefRef, DefRow, found};

#[derive(clap::Args)]
pub struct DefinitionGetArgs {
    /// The definition: SERVER/FOLDER/JOB, as definition list and job list
    /// print it
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
    let client = controlm.open(ctx, &args.at)?;
    let wanted = DefRef::parse(&args.id)?;
    let query = [
        ("format", "json".to_owned()),
        ("ctm", wanted.server.clone()),
        ("folder", wanted.folder.clone()),
        ("job", wanted.job.clone()),
    ];
    let answer = client.get(&with_query("deploy/jobs", &query))?;
    let id = format!("{}/{}/{}", wanted.server, wanted.folder, wanted.job);
    found(&answer, &wanted.server)
        .into_iter()
        .find(|found| found.row.id == id)
        .map(|found| DefDetail {
            row: found.row,
            definition: found.definition,
        })
        .ok_or_else(|| {
            Failure::not_found(format!("no job definition {id}"))
                .hint(format!(
                    "agent-cli controlm definition list --name {}",
                    wanted.job
                ))
                .into()
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
