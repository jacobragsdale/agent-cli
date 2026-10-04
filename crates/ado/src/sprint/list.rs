use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::Ado;
use crate::iteration::{iterations, team};

use super::SprintRow;

#[derive(clap::Args)]
pub struct SprintListArgs {
    /// The team whose sprints to list (default: [ado] team)
    #[arg(long)]
    team: Option<String>,
    /// Most rows to return (the latest)
    #[arg(long, default_value_t = 50)]
    limit: usize,
    /// The project (default: the first in [ado] project)
    #[arg(long)]
    project: Option<String>,
}

fn sprint_list(ctx: &Ctx, args: SprintListArgs) -> Result<Vec<SprintRow>> {
    let ado = Ado::load_in(ctx, args.project.as_deref())?;
    let team = team(&ado, args.team.as_deref())?;
    let mut sprints = iterations(ctx, &ado, team)?;
    // The undated and then the oldest are the ones to drop: an agent asks
    // about this sprint and the next, not last year's.
    if sprints.len() > args.limit {
        ctx.note(format!(
            "[the latest {} of {}; --limit N for more]",
            args.limit,
            sprints.len()
        ));
        let mut drop = sprints.len() - args.limit;
        sprints.retain(|sprint| {
            let keep = drop == 0 || sprint.start.is_some();
            drop -= usize::from(!keep);
            keep
        });
        sprints.drain(..drop);
    }
    Ok(sprints.into_iter().map(SprintRow::from).collect())
}

command! {
    pub SPRINT_LIST = ["ado", "sprint", "list"], Read,
    "List a team's sprints (iterations) with their dates, oldest first",
    keywords: ["iteration", "iterations", "sprints", "dates", "schedule", "cadence", "next", "previous"],
    example: "ado sprint list --fields id,start,finish,timeframe",
    run: sprint_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::sprint::tests::sprints;
    use crate::testing::{BASE, ado, urls};

    #[test]
    fn sprints_list_oldest_first_with_the_path_as_id_and_keep_the_latest() {
        let (outcome, transport) = ado(&["ado", "sprint", "list", "--limit", "2"], vec![sprints()]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "Fabrikam\\Sprint 12", "name": "Sprint 12", "path": "Fabrikam\\Sprint 12",
                    "start": "2026-09-22T00:00:00Z", "finish": "2026-10-05T00:00:00Z", "timeframe": "current"},
                {"id": "Fabrikam\\Sprint 13", "name": "Sprint 13", "path": "Fabrikam\\Sprint 13",
                    "start": "2099-03-02T00:00:00Z", "finish": "2099-03-13T00:00:00Z", "timeframe": "future"}
            ])
        );
        assert_eq!(outcome.stderr, "[the latest 2 of 3; --limit N for more]\n");
        assert_eq!(
            urls(&transport),
            [format!(
                "{BASE}/Fabrikam/Web%20Team/_apis/work/teamsettings/iterations?api-version=7.1"
            )]
        );
    }

    #[test]
    fn an_undated_iteration_is_dropped_before_the_current_sprint() {
        let dated = |n: u32, start: &str, timeframe: &str| {
            json!({"id": format!("i-{n}"), "name": format!("Sprint {n}"),
                "path": format!("Fabrikam\\Sprint {n}"),
                "attributes": {"startDate": format!("{start}T00:00:00Z"), "timeFrame": timeframe}})
        };
        let answer = Answer::json(&json!({"value": [
            dated(11, "2026-09-08", "past"),
            dated(12, "2026-09-22", "current"),
            {"id": "i-x", "name": "Someday", "path": "Fabrikam\\Someday",
                "attributes": {"timeFrame": "future"}},
        ]}));
        let (outcome, _) = ado(
            &["ado", "sprint", "list", "--limit", "1", "--fields", "name"],
            vec![answer],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!([{"name": "Sprint 12"}]));
    }
}
