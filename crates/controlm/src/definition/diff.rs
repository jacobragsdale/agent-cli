//! `controlm definition diff`: one job defined on two instances or servers
//! (qa's Control-M against prod's), or two jobs: command, host, run-as,
//! application, and the whole definition (schedule, the conditions it waits
//! for and adds, variables, notifications) key by key.

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Side, command, diff, scoped};
use anyhow::Result;

use crate::client::{At, ControlM};

#[derive(clap::Args)]
pub struct DefinitionDiffArgs {
    /// One side: [INSTANCE:]SERVER/FOLDER/JOB (qa:ctm-qa/NightlyLoads/load_orders)
    left: String,
    /// The other side, as left
    right: String,
    #[command(flatten)]
    at: At,
    #[command(flatten)]
    diff: DiffArgs,
}

const RULES: Rules = Rules {
    noise: &["id", "server"],
    ..Rules::PLAIN
};

fn definition_diff(ctx: &Ctx, args: DefinitionDiffArgs) -> Result<Diff> {
    let controlm = ControlM::load(ctx.config())?;
    let side = |raw: &str| -> Result<Side> {
        let (scope, rest) = scoped(raw);
        let at = At {
            instance: scope
                .map(str::to_owned)
                .or_else(|| args.at.instance.clone()),
        };
        let client = controlm.open(ctx, &at)?;
        let found = super::one(&client, rest)?;
        let mut row = serde_json::to_value(&found.row)?;
        row["definition"] = found.definition;
        Ok(Side::new(
            format!("{}:{}", client.instance.name, found.row.id),
            row,
        ))
    };
    let (left, right) = (side(&args.left)?, side(&args.right)?);
    diff(ctx, &args.diff, &RULES, left, right)
}

command! {
    pub DEFINITION_DIFF = ["controlm", "definition", "diff"], Read,
    "Compare a Control-M job defined in two places: command, host, schedule, waits",
    keywords: ["control-m", "ctm", "staging", "schedule", "conditions", "drift"],
    example: "controlm definition diff qa:ctm-qa/NightlyLoads/load_orders prod:ctm-prod/NightlyLoads/load_orders",
    run: definition_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{controlm_with, paths};

    const TWO: &str = "[[controlm.instance]]\nname = \"qa\"\n\
        base_url = \"https://ctm-qa.contoso.example:8443/automation-api\"\n\
        token_env = \"CONTROLM_TOKEN\"\n\
        [[controlm.instance]]\nname = \"prod\"\n\
        base_url = \"https://ctm.contoso.example:8443/automation-api\"\n\
        token_env = \"CONTROLM_TOKEN\"\n";

    fn folder(server: &str, host: &str, from: &str) -> Answer {
        Answer::json(&json!({"NightlyLoads": {
            "Type": "Folder", "ControlmServer": server,
            "load_orders": {"Type": "Job:Command", "Command": "/opt/etl/load_orders.sh",
                "Host": host, "RunAs": "etl", "When": {"FromTime": from, "WeekDays": ["ALL"]}}}}))
    }

    #[test]
    fn one_job_on_two_instances_differs_by_its_schedule_with_hosts_lined_up() {
        let (outcome, transport) = controlm_with(
            TWO,
            &[
                "controlm",
                "definition",
                "diff",
                "qa:ctm-qa/NightlyLoads/load_orders",
                "prod:ctm-prod/NightlyLoads/load_orders",
            ],
            vec![
                folder("ctm-qa", "etl-qa-01", "0100"),
                folder("ctm-prod", "etl-prod-01", "0030"),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["as"], json!(["qa=prod"]));
        assert_eq!(
            got["changes"],
            json!([{"path": "definition.When.FromTime", "left": "0100", "right": "0030"}])
        );
        assert!(
            paths(&transport)[0].starts_with("https://ctm-qa.contoso.example"),
            "{:?}",
            paths(&transport)
        );
    }
}
