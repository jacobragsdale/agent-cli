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
}

fn sprint_list(ctx: &Ctx, args: SprintListArgs) -> Result<Vec<SprintRow>> {
    let ado = Ado::load(ctx)?;
    let team = team(&ado, args.team.as_deref())?;
    let mut sprints = iterations(ctx, &ado, team)?;
    // The oldest are the ones to drop: an agent asks about this sprint and
    // the next, not last year's.
    if sprints.len() > args.limit {
        ctx.note(format!(
            "[the latest {} of {}; --limit N for more]",
            args.limit,
            sprints.len()
        ));
        sprints.drain(..sprints.len() - args.limit);
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
}
