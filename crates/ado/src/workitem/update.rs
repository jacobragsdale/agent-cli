use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::{Ado, Kind};
use crate::work_items::{WorkItemRow, moved_on, row, rows};

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
    /// Rank it just above this work item on the team's backlog
    #[arg(long, conflicts_with = "below")]
    above: Option<String>,
    /// Rank it just below this work item on the team's backlog
    #[arg(long)]
    below: Option<String>,
}

fn workitem_update(ctx: &Ctx, args: UpdateArgs) -> Result<WorkItemRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let changes = field_ops(ctx, &ado, args.title.as_deref(), &args.fields)?;
    let rank = rank(&ado, id, args.above.as_deref(), args.below.as_deref())?;
    if changes.is_empty() && rank.is_none() {
        return Err(Failure::usage("nothing to change")
            .hint(format!(
                "pass at least one of --state --assignee --title --iteration --area --priority --tags --description --acceptance-criteria --above --below, e.g. agent-cli ado workitem update {id} --state Active"
            ))
            .into());
    }
    let mut updated = None;
    if !changes.is_empty() {
        updated = Some(patch(ctx, &ado, id, args.if_rev, changes)?);
    }
    // After the fields, so a refused --if-rev leaves the rank alone.
    if let Some(order) = rank {
        let team = crate::iteration::team(&ado, args.fields.team.as_deref())?;
        let url = ado.team(team, "work/workitemsorder", "");
        ado.change(ctx, Effect::Write, Method::Patch, &url, order)?;
    }
    match updated {
        Some(updated) => Ok(updated),
        None => rows(ctx, &ado, &[id])?
            .pop()
            .ok_or_else(|| Failure::not_found(format!("work item {id} is gone")).into()),
    }
}

fn patch(
    ctx: &Ctx,
    ado: &Ado,
    id: i64,
    if_rev: Option<i64>,
    changes: Vec<Value>,
) -> Result<WorkItemRow> {
    // The test leads, so a work item that moved on refuses the whole document.
    let mut document: Vec<Value> = if_rev
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

/// The backlog move `--above` or `--below` asks for: a `ReorderOperation`
/// naming the other item as the next one or the previous one.
// ponytail: parentId 0 ranks it among the whole backlog; an item shown
// nested under its parent may need that parent's id here.
fn rank(ado: &Ado, id: i64, above: Option<&str>, below: Option<&str>) -> Result<Option<Value>> {
    let other = |raw| ado.id(Kind::WorkItem, raw);
    Ok(match (above, below) {
        (Some(above), _) => {
            Some(json!({"ids": [id], "parentId": 0, "previousId": 0, "nextId": other(above)?}))
        }
        (_, Some(below)) => {
            Some(json!({"ids": [id], "parentId": 0, "previousId": other(below)?, "nextId": 0}))
        }
        _ => None,
    })
}

command! {
    pub WORKITEM_UPDATE = ["ado", "workitem", "update"], Write,
    "Change a work item's state, assignee, title, iteration, tags or description",
    keywords: ["edit", "move", "reassign", "close", "resolve", "reopen", "set", "rev", "markdown", "rank", "reorder", "prioritize", "above", "below", "backlog"],
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

    #[test]
    fn above_and_below_rank_it_on_the_teams_backlog_after_any_field_change() {
        let order = format!("{BASE}/Fabrikam/Web%20Team/_apis/work/workitemsorder?api-version=7.1");
        let plans = dry_run(
            &["ado", "workitem", "update", "42", "--above", "#7"],
            vec![],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(plans[0]["url"], order);
        assert_eq!(
            plans[0]["body"],
            json!({"ids": [42], "parentId": 0, "previousId": 0, "nextId": 7})
        );

        let (outcome, transport) = ado(
            &[
                "ado", "workitem", "update", "42", "--state", "Active", "--below", "7",
            ],
            vec![
                Answer::json(&item(42, 8, "Crash")),
                Answer::json(&json!({"count": 1, "value": [{"id": 42, "order": 1000102770.0}]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["rev"], 8);
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            format!("{BASE}/_apis/wit/workitems/42?api-version=7.1")
        );
        assert_eq!(sent[1].url, order);
        assert_eq!(
            sent[1].body.as_ref().unwrap(),
            &json!({"ids": [42], "parentId": 0, "previousId": 7, "nextId": 0})
        );

        let (outcome, transport) = ado(
            &[
                "ado", "workitem", "update", "42", "--above", "7", "--below", "9",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn iteration_macros_become_the_named_teams_sprint_path() {
        let (outcome, transport) = crate::testing::ado(
            &[
                "ado",
                "workitem",
                "update",
                "42",
                "--iteration",
                "@next",
                "--team",
                "Data",
                "--dry-run",
            ],
            vec![crate::sprint::tests::sprints()],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent()[0].url,
            format!("{BASE}/Fabrikam/Data/_apis/work/teamsettings/iterations?api-version=7.1")
        );
        assert_eq!(
            outcome.json()["would"][0]["body"],
            json!([{"op": "add", "path": "/fields/System.IterationPath", "value": "Fabrikam\\Sprint 13"}])
        );
    }
}
