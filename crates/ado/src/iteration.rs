//! Teams' sprints: which team a command means, the team's iterations, and
//! what `@current`, `@next`, `@previous`, a path or a sprint name stands for.

use std::time::Duration;

use agent_cli_core::{Ctx, Failure, pick};
use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::client::{Ado, list, stamp, text};

/// Sprints change at their boundaries, so an hour keeps `@current` close.
const CACHE_TTL: Duration = Duration::from_secs(3600);

/// A sprint as the team's settings describe it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Iteration {
    pub(crate) id: String,
    pub(crate) name: String,
    /// `Project\Sprint 12`, what `System.IterationPath` holds.
    pub(crate) path: String,
    pub(crate) start: Option<String>,
    pub(crate) finish: Option<String>,
    /// `past`, `current` or `future`, as Azure DevOps judges it.
    pub(crate) timeframe: Option<String>,
}

/// The team a command means: the one named, else the one `[ado] team` holds.
pub(crate) fn team<'a>(ado: &'a Ado, wanted: Option<&'a str>) -> Result<&'a str> {
    if let Some(team) = wanted {
        return Ok(team);
    }
    if ado.teams.is_empty() {
        return Err(no_team());
    }
    Ok(pick("team", "--team", None, &ado.teams, String::as_str)?)
}

/// `[ado] team` unset, where a command needs a team.
pub(crate) fn no_team() -> anyhow::Error {
    Failure::setup("[ado] team is not set, and sprints and members belong to a team")
        .hint("agent-cli ado team list --fields name, then set team = \"NAME\" under [ado] (or AGENT_CLI_ADO_TEAM)")
        .into()
}

/// The team's iterations in its settings' order (by start date), cached for
/// an hour.
pub(crate) fn iterations(ctx: &Ctx, ado: &Ado, team: &str) -> Result<Vec<Iteration>> {
    let key = ado.cache_key(&format!(
        "iterations:{}:{}",
        ado.project.to_lowercase(),
        team.to_lowercase()
    ));
    if let Some(iterations) = ctx.cache().get(&key) {
        return Ok(iterations);
    }
    let answer = ado.get(ctx, &ado.team(team, "work/teamsettings/iterations", ""))?;
    let iterations: Vec<Iteration> = list(&answer["value"])
        .iter()
        .filter_map(|iteration| {
            let attributes = &iteration["attributes"];
            Some(Iteration {
                id: text(&iteration["id"])?,
                name: text(&iteration["name"])?,
                path: text(&iteration["path"])?,
                start: stamp(&attributes["startDate"]),
                finish: stamp(&attributes["finishDate"]),
                timeframe: text(&attributes["timeFrame"]),
            })
        })
        .collect();
    ctx.cache().put(&key, &iterations, CACHE_TTL);
    Ok(iterations)
}

/// The sprint `raw` names for `team` (else `[ado] team`): `@current`,
/// `@next`, `@previous`, a full path, or a name unique among the team's.
pub(crate) fn resolve(ctx: &Ctx, ado: &Ado, team: Option<&str>, raw: &str) -> Result<Iteration> {
    let team = self::team(ado, team)?;
    let raw = raw.trim();
    let all = iterations(ctx, ado, team)?;
    let timeframe =
        |iteration: &&Iteration, wanted: &str| iteration.timeframe.as_deref() == Some(wanted);
    let found = match raw.to_ascii_lowercase().as_str() {
        "@current" => all.iter().find(|i| timeframe(i, "current")),
        // Between sprints there is no current one, so next and previous go
        // by the future and the past rather than by the current one's place.
        "@next" => all.iter().find(|i| timeframe(i, "future")),
        "@previous" => all.iter().rev().find(|i| timeframe(i, "past")),
        _ => {
            let matching: Vec<&Iteration> = all
                .iter()
                .filter(|i| i.path.eq_ignore_ascii_case(raw) || i.name.eq_ignore_ascii_case(raw))
                .collect();
            if let [_, _, ..] = matching.as_slice() {
                let paths: Vec<&str> = matching.iter().map(|i| i.path.as_str()).collect();
                return Err(Failure::usage(format!(
                    "{raw:?} names {} of {team}'s sprints: {}",
                    matching.len(),
                    paths.join(", ")
                ))
                .hint("give the sprint's full path instead")
                .into());
            }
            matching.first().copied()
        }
    };
    found.cloned().ok_or_else(|| {
        let names: Vec<&str> = all.iter().map(|i| i.name.as_str()).collect();
        Failure::not_found(format!(
            "{team} has no sprint {raw:?}; its sprints are: {}",
            if names.is_empty() {
                "none".to_owned()
            } else {
                names.join(", ")
            }
        ))
        .into()
    })
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, FakeTransport, ctx};
    use agent_cli_core::{Config, Exit, Setup};
    use serde_json::json;

    use super::*;

    fn sprint(id: &str, name: &str, start: &str, timeframe: &str) -> serde_json::Value {
        json!({"id": id, "name": name, "path": format!("Fabrikam\\{name}"),
            "attributes": {"startDate": format!("{start}T00:00:00Z"),
                "finishDate": format!("{start}T23:59:59Z"), "timeFrame": timeframe}})
    }

    fn sprints() -> Answer {
        Answer::json(&json!({"count": 4, "value": [
            sprint("i-11", "Sprint 11", "2026-09-08", "past"),
            sprint("i-12", "Sprint 12", "2026-09-22", "current"),
            sprint("i-13", "Sprint 13", "2026-10-06", "future"),
            sprint("i-14", "Sprint 14", "2026-10-20", "future"),
        ]}))
    }

    fn ado(config: &str) -> Ado {
        Ado::names(&Config::parse("c.toml", Some(config), Vec::new())).unwrap()
    }

    fn resolved(
        answers: Vec<Answer>,
        config: &str,
        team: Option<&str>,
        raw: &str,
    ) -> Result<Iteration> {
        let dir = tempfile::tempdir().unwrap();
        let setup = Setup {
            cache_dir: Some(dir.path().to_owned()),
            ..Setup::fake(FakeTransport::answering(answers))
        };
        resolve(&ctx(setup), &ado(config), team, raw)
    }

    const ONE_TEAM: &str =
        "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\nteam = \"Web Team\"\n";

    #[test]
    fn macros_paths_and_names_resolve_against_the_teams_sprints_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let transport = FakeTransport::answering([sprints()]);
        let setup = Setup {
            cache_dir: Some(dir.path().to_owned()),
            ..Setup::fake(transport.clone())
        };
        let ctx = ctx(setup);
        let ado = ado(ONE_TEAM);
        let path = |raw: &str| resolve(&ctx, &ado, None, raw).unwrap().path;
        assert_eq!(path("@current"), "Fabrikam\\Sprint 12");
        assert_eq!(path("@NEXT"), "Fabrikam\\Sprint 13");
        assert_eq!(path("@previous"), "Fabrikam\\Sprint 11");
        assert_eq!(path("fabrikam\\sprint 14"), "Fabrikam\\Sprint 14");
        let named = resolve(&ctx, &ado, None, "Sprint 13").unwrap();
        assert_eq!(named.id, "i-13");
        assert_eq!(named.start.as_deref(), Some("2026-10-06T00:00:00Z"));
        assert_eq!(transport.sent().len(), 1, "cached for the hour");
        assert_eq!(
            transport.sent()[0].url,
            "https://dev.azure.com/contoso/Fabrikam/Web%20Team/_apis/work/teamsettings/iterations?api-version=7.1"
        );
    }

    #[test]
    fn between_sprints_next_and_previous_still_resolve_and_current_is_not_found() {
        let answer = || {
            Answer::json(&json!({"value": [
                sprint("i-12", "Sprint 12", "2026-09-22", "past"),
                sprint("i-13", "Sprint 13", "2026-10-06", "future"),
            ]}))
        };
        assert_eq!(
            resolved(vec![answer()], ONE_TEAM, None, "@next")
                .unwrap()
                .id,
            "i-13"
        );
        assert_eq!(
            resolved(vec![answer()], ONE_TEAM, None, "@previous")
                .unwrap()
                .id,
            "i-12"
        );
        let error = resolved(vec![answer()], ONE_TEAM, None, "@current").unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.exit, Exit::NotFound);
        assert!(
            failure.message.contains("Sprint 12, Sprint 13"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn the_team_is_the_one_named_else_the_only_one_configured() {
        let several =
            "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\nteam = [\"Web\", \"Data\"]\n";
        assert_eq!(
            resolved(vec![sprints()], several, Some("Data"), "@current")
                .unwrap()
                .id,
            "i-12"
        );
        let error = resolved(vec![], several, None, "@current").unwrap_err();
        assert_eq!(error.downcast_ref::<Failure>().unwrap().exit, Exit::Usage);
        let none = "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\n";
        let error = resolved(vec![], none, None, "@current").unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.exit, Exit::Setup);
        assert!(
            failure
                .hint
                .as_deref()
                .unwrap()
                .starts_with("agent-cli ado team list")
        );
    }

    #[test]
    fn a_name_two_sprints_share_is_a_usage_error_naming_their_paths() {
        let answer = Answer::json(&json!({"value": [
            {"id": "a", "name": "Sprint 1", "path": "Fabrikam\\Web\\Sprint 1", "attributes": {}},
            {"id": "b", "name": "Sprint 1", "path": "Fabrikam\\Data\\Sprint 1", "attributes": {}},
        ]}));
        let error = resolved(vec![answer], ONE_TEAM, None, "sprint 1").unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.exit, Exit::Usage);
        assert!(
            failure
                .message
                .contains("Fabrikam\\Web\\Sprint 1, Fabrikam\\Data\\Sprint 1"),
            "{}",
            failure.message
        );
    }
}
