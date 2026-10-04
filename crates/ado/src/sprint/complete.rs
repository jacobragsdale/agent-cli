use std::collections::HashMap;

use agent_cli_core::{Ctx, Effect, Exit, Failure, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{API, Ado, Body};
use crate::ids::arg;
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
    /// The project (default: the first in [ado] project)
    #[arg(long)]
    project: Option<String>,
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
    let ado = Ado::load_in(ctx, args.project.as_deref())?;
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
    let (work, mut read) = items(ctx, &ado, team, &sprint, &["System.Title", PARENT])?;
    // One rule for every parent: one planned elsewhere is in the sprint's
    // list, and a removed one, which the list leaves out, is read here.
    let mut missing: Vec<i64> = (work.iter())
        .filter_map(|item| item["fields"][PARENT].as_i64())
        .filter(|parent| !read.contains_key(parent))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    if !missing.is_empty() {
        let fields = ["System.WorkItemType", "System.State"];
        for parent in crate::work_items::read(ctx, &ado, &missing, &fields)? {
            read.extend(parent["id"].as_i64().map(|id| (id, parent)));
        }
    }
    let mut seen = HashMap::new();
    // Every finished item read, with its state.
    let mut finished: HashMap<i64, String> = HashMap::new();
    for (id, item) in &read {
        if done(ctx, &ado, &mut seen, item)? {
            let state = item["fields"]["System.State"].as_str().unwrap_or_default();
            finished.insert(*id, state.to_owned());
        }
    }
    let mut moved = Vec::new();
    let mut skipped = Vec::new();
    // Like `each`: every item has its turn, so a dry run plans every move and
    // one refused item does not strand the rest.
    let mut stopped: Option<anyhow::Error> = None;
    // The deadline ends the turns: the move under way may or may not have
    // been made, and every later one would time out too.
    let mut cut: Option<i64> = None;
    for item in &work {
        let Some(id) = item["id"].as_i64() else {
            continue;
        };
        if finished.contains_key(&id) {
            continue;
        }
        let parent = item["fields"][PARENT].as_i64();
        if let Some((parent, state)) = parent.and_then(|p| Some((p, finished.get(&p)?))) {
            skipped.push(Skipped {
                id,
                reason: format!("its parent {parent} is {state}"),
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
                    reason: CHANGED.to_owned(),
                }),
                Some(failure) if failure.exit == Exit::TimedOut => {
                    cut = Some(id);
                    break;
                }
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
    // Run again, it moves what is still unfinished in the sprint.
    let again = format!(
        "agent-cli ado sprint complete {} --move-to {} --yes{}",
        arg(&sprint.path),
        arg(&to.path),
        (args.team.as_deref()).map_or_else(String::new, |team| format!(" --team {}", arg(team)))
    );
    let completed = Completed {
        sprint: sprint.path,
        to: to.path,
        moved,
        skipped,
    };
    if let Some(id) = cut {
        return Err(Failure::timed_out(format!(
            "the deadline came while moving {id}, which may or may not have moved; the items after it were not tried"
        ))
        .hint(again)
        .with_data(&completed)
        .into());
    }
    if let Some(error) = stopped {
        return Err(Failure::new(Exit::Failed, format!("{error:#}"))
            .hint(again)
            .with_data(&completed)
            .into());
    }
    if completed.skipped.iter().any(|skip| skip.reason == CHANGED) {
        ctx.note(format!("[next: {again}]"));
    }
    Ok(completed)
}

const CHANGED: &str = "it changed since it was read";
const PARENT: &str = "System.Parent";

command! {
    pub SPRINT_COMPLETE = ["ado", "sprint", "complete"], Destructive,
    "Close a sprint: move its unfinished work items to the next sprint",
    keywords: ["rollover", "roll", "carry", "over", "move", "unfinished", "incomplete", "end", "close", "iteration"],
    example: "ado sprint complete @current --move-to @next --yes",
    run: sprint_complete,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, FakeTransport};
    use agent_cli_core::{Failure, Method, Setup};
    use serde_json::{Value, json};

    use crate::sprint::tests::{relations, sprints, states};
    use crate::testing::{BASE, ado, batch, dry_run};

    fn item(id: i64, kind: &str, state: &str, rev: i64) -> Value {
        json!({"id": id, "rev": rev, "fields": {"System.WorkItemType": kind, "System.State": state,
            "System.Title": format!("Item {id}"), "System.IterationPath": "Fabrikam\\Sprint 12"}})
    }

    fn child(id: i64, state: &str, rev: i64, parent: i64) -> Value {
        let mut task = item(id, "Task", state, rev);
        task["fields"]["System.Parent"] = json!(parent);
        task
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
                child(2, "New", 2, 1),
                item(3, "User Story", "Closed", 9),
                child(4, "Active", 3, 3),
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
                    {"id": 4, "reason": "its parent 3 is Closed"}
                ]
            })
        );
        assert_eq!(transport.remaining(), 0);
        assert_eq!(
            outcome.stderr,
            "[next: agent-cli ado sprint complete 'Fabrikam\\Sprint 12' --move-to 'Fabrikam\\Sprint 13' --yes]\n"
        );

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

    /// The fake, but the second PATCH outlives the deadline.
    struct Late(FakeTransport);

    impl agent_cli_core::Transport for Late {
        fn send(
            &self,
            request: &agent_cli_core::Request<'_>,
            authorization: Option<&agent_cli_core::Secret>,
            timeout: std::time::Duration,
        ) -> anyhow::Result<agent_cli_core::Response> {
            let patches = self
                .0
                .sent()
                .iter()
                .filter(|sent| sent.method == Method::Patch)
                .count();
            if request.method == Method::Patch && patches == 1 {
                return Err(Failure::timed_out("PATCH did not answer before the deadline").into());
            }
            self.0.send(request, authorization, timeout)
        }
    }

    #[test]
    fn the_deadline_stops_the_moves_and_exits_124_with_what_moved() {
        let mut answers = reads();
        answers.push(Answer::json(&item(1, "User Story", "Active", 5)));
        let fake = FakeTransport::answering(answers);
        let setup = Setup::fake(Late(fake.clone())).with_config(crate::testing::CONFIG);
        let argv = ["ado", "sprint", "complete", "@current", "--yes"];
        let outcome = agent_cli_core::testing::run(&[crate::DOMAIN], &argv, setup);
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert_eq!(
            outcome.json()["moved"],
            json!([{"id": 1, "type": "User Story", "title": "Item 1", "state": "Active"}])
        );
        assert!(
            outcome.stderr.contains("the deadline came while moving 2, which may or may not have moved; the items after it were not tried"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("hint: agent-cli ado sprint complete 'Fabrikam\\Sprint 12' --move-to 'Fabrikam\\Sprint 13' --yes"),
            "{}",
            outcome.stderr
        );
        let patches = fake
            .sent()
            .iter()
            .filter(|sent| sent.method == Method::Patch)
            .count();
        assert_eq!(patches, 1, "item 5 was not tried");
    }

    #[test]
    fn a_child_of_a_parent_finished_elsewhere_or_removed_is_skipped_too() {
        // Task 6's story 7 is closed in another sprint (the list names it);
        // task 8's story 9 was removed, which the list leaves out.
        let mut elsewhere = item(7, "User Story", "Closed", 3);
        elsewhere["fields"]["System.IterationPath"] = json!("Fabrikam\\Sprint 11");
        let answers = vec![
            sprints(),
            sprints(),
            relations(&[(7, None), (6, Some(7)), (8, None)]),
            batch(vec![
                elsewhere,
                child(6, "New", 2, 7),
                child(8, "New", 2, 9),
            ]),
            batch(vec![
                json!({"id": 9, "fields": {"System.WorkItemType": "User Story", "System.State": "Removed"}}),
            ]),
            states(),
            states(),
            states(),
        ];
        let (outcome, transport) =
            ado(&["ado", "sprint", "complete", "@current", "--yes"], answers);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["skipped"],
            json!([{"id": 6, "reason": "its parent 7 is Closed"}, {"id": 8, "reason": "its parent 9 is Removed"}])
        );
        assert_eq!(transport.remaining(), 0, "nothing moved");
    }
}
