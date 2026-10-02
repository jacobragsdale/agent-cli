use std::collections::HashMap;

use agent_cli_core::{Ctx, Effect, Exit, Failure, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{API, Ado, Body};
use crate::iteration::{resolve, team};
use crate::work_items::{WorkItemRef, moved_on, row};

use super::{done, items};

#[derive(clap::Args)]
pub struct SprintCompleteArgs {
    /// The sprint to close: @current, @previous, a sprint's path or its name
    sprint: String,
    /// The sprint unfinished work moves to: @next (the default), a path or a name
    #[arg(long, default_value = "@next")]
    move_to: String,
    /// The team whose sprints they are (default: [ado] team)
    #[arg(long)]
    team: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Completed {
    /// The closed sprint's path.
    sprint: String,
    /// The path its unfinished work moved to.
    to: String,
    moved: Vec<WorkItemRef>,
    skipped: Vec<Skipped>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Skipped {
    id: i64,
    reason: String,
}

fn sprint_complete(ctx: &Ctx, args: SprintCompleteArgs) -> Result<Completed> {
    let ado = Ado::load(ctx)?;
    let team = team(&ado, args.team.as_deref())?;
    let sprint = resolve(ctx, &ado, Some(team), &args.sprint)?;
    let to = resolve(ctx, &ado, Some(team), &args.move_to)?;
    if to.path.eq_ignore_ascii_case(&sprint.path) {
        return Err(Failure::usage(format!(
            "--move-to names {}, the sprint being closed",
            sprint.path
        ))
        .hint("agent-cli ado sprint list --fields id,timeframe")
        .into());
    }
    let (work, parents) = items(ctx, &ado, team, &sprint, &["System.Title"])?;
    let mut seen = HashMap::new();
    let mut finished: Vec<i64> = Vec::new();
    for item in &work {
        if done(ctx, &ado, &mut seen, item)? {
            finished.extend(item["id"].as_i64());
        }
    }
    let mut moved = Vec::new();
    let mut skipped = Vec::new();
    // Like `each`: every item has its turn, so a dry run plans every move and
    // one refused item does not strand the rest.
    let mut stopped: Option<anyhow::Error> = None;
    for item in &work {
        let Some(id) = item["id"].as_i64() else {
            continue;
        };
        if finished.contains(&id) {
            continue;
        }
        if let Some(parent) = parents.get(&id).filter(|p| finished.contains(p)) {
            skipped.push(Skipped {
                id,
                reason: format!("its parent {parent} is done"),
            });
            continue;
        }
        let document = json!([
            {"op": "test", "path": "/rev", "value": item["rev"]},
            {"op": "add", "path": "/fields/System.IterationPath", "value": to.path},
        ]);
        let url = ado.api(None, &format!("wit/workitems/{id}"), "", API);
        let sent = ado
            .send(
                ctx,
                Method::Patch,
                &url,
                Body::Patch(document),
                Some(Effect::Destructive),
            )
            .and_then(|response| response.json())
            .map_err(|error| moved_on(error, id));
        match sent {
            Ok(_) => moved.push(WorkItemRef::from(&row(item))),
            Err(error) => match error.downcast_ref::<Failure>() {
                Some(failure) if failure.exit == Exit::Conflict => skipped.push(Skipped {
                    id,
                    reason: "it changed since it was read".to_owned(),
                }),
                Some(failure) => skipped.push(Skipped {
                    id,
                    reason: failure.message.clone(),
                }),
                None => {
                    stopped.get_or_insert(error);
                }
            },
        }
    }
    if let Some(error) = stopped {
        return Err(error);
    }
    Ok(Completed {
        sprint: sprint.path,
        to: to.path,
        moved,
        skipped,
    })
}

command! {
    pub SPRINT_COMPLETE = ["ado", "sprint", "complete"], Destructive,
    "Close a sprint: move its unfinished work items to the next sprint",
    keywords: ["rollover", "roll", "carry", "over", "move", "unfinished", "incomplete", "end", "close", "iteration"],
    example: "ado sprint complete @current --move-to @next --yes",
    run: sprint_complete,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::sprint::tests::{relations, sprints, states};
    use crate::testing::{BASE, ado, batch, dry_run};

    fn item(id: i64, kind: &str, state: &str, rev: i64) -> Value {
        json!({"id": id, "rev": rev, "fields": {"System.WorkItemType": kind, "System.State": state,
            "System.Title": format!("Item {id}"), "System.IterationPath": "Fabrikam\\Sprint 12"}})
    }

    /// The reads before the first move: the sprints (once for each sprint named), the sprint's items
    /// (story 1 open with task 2; story 3 closed with open task 4; task 5
    /// open), the batch, and the states of each type and state.
    fn reads() -> Vec<Answer> {
        vec![
            sprints(),
            sprints(),
            relations(&[(1, None), (2, Some(1)), (3, None), (4, Some(3)), (5, None)]),
            batch(vec![
                item(1, "User Story", "Active", 4),
                item(2, "Task", "New", 2),
                item(3, "User Story", "Closed", 9),
                item(4, "Task", "Active", 3),
                item(5, "Task", "Active", 7),
            ]),
            states(),
            states(),
            states(),
            states(),
        ]
    }

    #[test]
    fn a_dry_run_plans_one_tested_patch_per_unfinished_item_and_sends_none() {
        let plans = dry_run(&["ado", "sprint", "complete", "@current"], reads());
        let urls: Vec<&str> = plans
            .iter()
            .map(|plan| plan["url"].as_str().unwrap())
            .collect();
        assert_eq!(
            urls,
            [1, 2, 5].map(|id| format!("{BASE}/_apis/wit/workitems/{id}?api-version=7.1"))
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["body"],
            json!([
                {"op": "test", "path": "/rev", "value": 4},
                {"op": "add", "path": "/fields/System.IterationPath", "value": "Fabrikam\\Sprint 13"}
            ])
        );
    }

    #[test]
    fn completing_moves_what_is_open_and_skips_a_done_parents_children_and_changed_items() {
        let mut answers = reads();
        answers.extend([
            Answer::json(&item(1, "User Story", "Active", 5)),
            Answer::status(
                400,
                r#"{"message":"TF401289: The test operation failed for path /rev: expected 2, found 3."}"#,
            ),
            Answer::json(&item(5, "Task", "Active", 8)),
        ]);
        let (outcome, transport) = ado(
            &[
                "ado",
                "sprint",
                "complete",
                "Sprint 12",
                "--move-to",
                "Sprint 13",
                "--yes",
            ],
            answers,
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({
                "sprint": "Fabrikam\\Sprint 12", "to": "Fabrikam\\Sprint 13",
                "moved": [
                    {"id": 1, "type": "User Story", "title": "Item 1", "state": "Active"},
                    {"id": 5, "type": "Task", "title": "Item 5", "state": "Active"}
                ],
                "skipped": [
                    {"id": 2, "reason": "it changed since it was read"},
                    {"id": 4, "reason": "its parent 3 is done"}
                ]
            })
        );
        assert_eq!(transport.remaining(), 0);

        let (outcome, transport) = ado(&["ado", "sprint", "complete", "@current"], vec![]);
        assert_eq!(outcome.code, 2, "a bulk move needs --yes: {outcome:?}");
        assert!(transport.sent().is_empty());

        let (outcome, _) = ado(
            &[
                "ado",
                "sprint",
                "complete",
                "@current",
                "--move-to",
                "Sprint 12",
                "--yes",
            ],
            vec![sprints(), sprints()],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("the sprint being closed"),
            "{}",
            outcome.stderr
        );
    }
}
