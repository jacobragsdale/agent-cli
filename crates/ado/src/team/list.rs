use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, list, segment, text};

#[derive(clap::Args)]
pub struct TeamListArgs {
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
    /// The project (default: the first in [ado] project)
    #[arg(long)]
    project: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TeamRow {
    name: String,
    id: String,
    description: Option<String>,
}

fn team_list(ctx: &Ctx, args: TeamListArgs) -> Result<Vec<TeamRow>> {
    let ado = Ado::load_in(ctx, args.project.as_deref())?;
    let url = ado.api(
        None,
        &format!("projects/{}/teams", segment(&ado.project)),
        &format!("$top={}", args.limit.saturating_add(1)),
        crate::client::API,
    );
    let answer = ado.get(ctx, &url)?;
    let mut teams: Vec<TeamRow> = list(&answer["value"])
        .iter()
        .filter_map(|team| {
            Some(TeamRow {
                name: text(&team["name"])?,
                id: text(&team["id"])?,
                description: text(&team["description"]),
            })
        })
        .collect();
    if teams.len() > args.limit {
        teams.truncate(args.limit);
        ctx.note(format!("[first {}; --limit N for more]", args.limit));
    }
    Ok(teams)
}

command! {
    pub TEAM_LIST = ["ado", "team", "list"], Read,
    "List the project's teams (for [ado] team, which @current needs)",
    keywords: ["teams", "squad", "group", "sprint", "iteration", "setup"],
    example: "ado team list --fields name",
    run: team_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, urls};

    #[test]
    fn team_list_names_the_teams_and_says_when_there_are_more() {
        let (outcome, transport) = ado(
            &["ado", "team", "list", "--limit", "1"],
            vec![Answer::json(&json!({"count": 2, "value": [
                {"id": "t-1", "name": "Web Team", "description": "The web app"},
                {"id": "t-2", "name": "Data", "description": ""}
            ]}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "Web Team", "id": "t-1", "description": "The web app"}])
        );
        assert_eq!(outcome.stderr, "[first 1; --limit N for more]\n");
        assert_eq!(
            urls(&transport),
            [format!(
                "{BASE}/_apis/projects/Fabrikam/teams?$top=2&api-version=7.1"
            )]
        );
    }
}
