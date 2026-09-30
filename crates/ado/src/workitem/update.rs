use agent_cli_core::{Ctx, Failure, Method, command};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::{Ado, Kind};
use crate::work_items::{WorkItemRow, moved_on, row};

use super::{Fields, field_ops};

#[derive(clap::Args)]
pub struct UpdateArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// A new title
    #[arg(long)]
    title: Option<String>,
    #[command(flatten)]
    fields: Fields,
    /// Refuse unless it is still at this rev (from workitem get)
    #[arg(long)]
    if_rev: Option<i64>,
}

fn workitem_update(ctx: &Ctx, args: UpdateArgs) -> Result<WorkItemRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let changes = field_ops(ctx, &ado, args.title.as_deref(), &args.fields)?;
    if changes.is_empty() {
        return Err(Failure::usage("nothing to change")
            .hint(format!(
                "pass at least one of --state --assignee --title --iteration --area --priority --tags --description --acceptance-criteria, e.g. agent-cli ado workitem update {id} --state Active"
            ))
            .into());
    }
    // The test leads, so a work item that moved on refuses the whole document.
    let mut document: Vec<Value> = args
        .if_rev
        .map(|rev| json!({"op": "test", "path": "/rev", "value": rev}))
        .into_iter()
        .collect();
    document.extend(changes);
    let url = ado.api(None, &format!("wit/workitems/{id}"), "", crate::client::API);
    let updated = ado
        .patch_work_item(ctx, Method::Patch, &url, document)
        .map_err(|error| moved_on(error, id))?;
    Ok(row(&updated))
}

command! {
    pub WORKITEM_UPDATE = ["ado", "workitem", "update"], Write,
    "Change a work item's state, assignee, title, iteration, tags or description",
    keywords: ["edit", "move", "reassign", "close", "resolve", "reopen", "set", "rev", "markdown"],
    example: "ado workitem update 42 --state Active --assignee @me --if-rev 7",
    run: workitem_update,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, dry_run, item};

    #[test]
    fn update_leads_with_the_rev_test_and_a_moved_on_item_is_a_conflict() {
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "update",
                "42",
                "--state",
                "Resolved",
                "--assignee",
                "",
                "--if-rev",
                "7",
            ],
            vec![],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            format!("{BASE}/_apis/wit/workitems/42?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!([
                {"op": "test", "path": "/rev", "value": 7},
                {"op": "add", "path": "/fields/System.State", "value": "Resolved"},
                {"op": "remove", "path": "/fields/System.AssignedTo"}
            ])
        );

        for refusal in [
            Answer::status(
                400,
                r#"{"message":"TF401289: The test operation failed for path /rev: expected 7, found 9."}"#,
            ),
            Answer::status(412, r#"{"message":"precondition failed"}"#),
        ] {
            let (outcome, _) = ado(
                &[
                    "ado", "workitem", "update", "42", "--title", "New", "--if-rev", "7",
                ],
                vec![refusal],
            );
            assert_eq!(outcome.code, 5, "{outcome:?}");
            assert!(
                outcome
                    .stderr
                    .starts_with("error: work item 42 changed since it was read"),
                "{}",
                outcome.stderr
            );
            assert!(
                outcome
                    .stderr
                    .contains("hint: re-read it (agent-cli ado workitem get 42"),
                "{}",
                outcome.stderr
            );
        }

        let (outcome, _) = ado(
            &["ado", "workitem", "update", "42", "--state", "Nope"],
            vec![Answer::status(
                400,
                r#"{"message":"TF401320: Rule error for field State."}"#,
            )],
        );
        assert_eq!(
            outcome.code, 2,
            "any other refusal keeps its code: {outcome:?}"
        );

        let (outcome, transport) = ado(&["ado", "workitem", "update", "42"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("nothing to change"));
        assert!(transport.sent().is_empty());

        let (outcome, _) = ado(
            &["ado", "workitem", "update", "42", "--priority", "1"],
            vec![Answer::json(&item(42, 8, "Crash"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["rev"], 8);
    }
}
