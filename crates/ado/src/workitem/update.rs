use agent_cli_core::{Ctx, Failure, Method, command};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::{Ado, Kind};
use crate::work_items::{WorkItemRow, moved_on, row};

use super::{Comment, Fields, broke_rules, field_ops};

#[derive(clap::Args)]
pub struct UpdateArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// A new title
    #[arg(long)]
    title: Option<String>,
    #[command(flatten)]
    fields: Fields,
    #[command(flatten)]
    comment: Comment,
    /// Refuse unless it is still at this rev (from workitem get)
    #[arg(long)]
    if_rev: Option<i64>,
}

fn workitem_update(ctx: &Ctx, args: UpdateArgs) -> Result<WorkItemRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let kind = || item_type(ctx, &ado, id);
    let changes = field_ops(
        ctx,
        &ado,
        args.title.as_deref(),
        &args.fields,
        Some(&args.comment),
        &kind,
    )?;
    if changes.is_empty() {
        return Err(Failure::usage("nothing to change")
            .hint(format!(
                "pass at least one of --state --assignee --title --iteration --area --priority --tags --description --acceptance-criteria --field --comment, e.g. agent-cli ado workitem update {id} --state Active"
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
        .map_err(|error| moved_on(error, id))
        .map_err(|error| broke_rules(error, || kind().unwrap_or_else(|_| "TYPE".to_owned())))?;
    Ok(row(&updated))
}

/// The work item's type, whose fields `--field` names.
fn item_type(ctx: &Ctx, ado: &Ado, id: i64) -> Result<String> {
    let url = ado.api(
        None,
        &format!("wit/workitems/{id}"),
        "fields=System.WorkItemType",
        crate::client::API,
    );
    let item = ado.get(ctx, &url)?;
    crate::client::text(&item["fields"]["System.WorkItemType"])
        .ok_or_else(|| anyhow::anyhow!("work item {id} came back without its type"))
}

command! {
    pub WORKITEM_UPDATE = ["ado", "workitem", "update"], Write,
    "Change a work item's state, assignee, fields or description, with a comment",
    keywords: ["edit", "move", "reassign", "take", "close", "resolve", "reopen", "set", "custom", "field", "points", "rev", "markdown"],
    example: "ado workitem update 42 --assignee @me --comment 'Taking this, @<Sam Lee>' --if-rev 7",
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
            vec![
                Answer::status(
                    400,
                    r#"{"message":"TF401326: Invalid field status 'InvalidListValue' for field 'System.State'."}"#,
                ),
                Answer::json(&json!({"id": 42, "fields": {"System.WorkItemType": "User Story"}})),
            ],
        );
        assert_eq!(outcome.code, 2, "a broken rule keeps its code: {outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("answered 400: TF401326: Invalid field status")
                && outcome
                    .stderr
                    .contains("hint: agent-cli ado workitem-type get \"User Story\""),
            "{}",
            outcome.stderr
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
