use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, list, segment, text};

#[derive(clap::Args)]
pub struct PersonListArgs {
    /// A team's name (default: every team in [ado] team)
    #[arg(long)]
    team: Option<String>,
    /// Words in the name or address
    #[arg(long)]
    text: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PersonRow {
    /// Their sign-in address, which --assignee and mentions take.
    id: String,
    name: String,
    team: String,
}

fn person_list(ctx: &Ctx, args: PersonListArgs) -> Result<Vec<PersonRow>> {
    let ado = Ado::load(ctx)?;
    let teams: Vec<&str> = match &args.team {
        Some(team) => vec![team.as_str()],
        None if ado.teams.is_empty() => return Err(crate::iteration::no_team()),
        None => ado.teams.iter().map(String::as_str).collect(),
    };
    let words = args.text.as_deref().map(str::to_lowercase);
    let mut people: Vec<PersonRow> = Vec::new();
    for team in teams {
        // ponytail: one page of 1,000 members a team; page with $skip if a
        // team outgrows it.
        let url = ado.api(
            None,
            &format!(
                "projects/{}/teams/{}/members",
                segment(&ado.project),
                segment(team)
            ),
            "$top=1000",
            crate::client::API,
        );
        let answer = ado.get(ctx, &url)?;
        for member in list(&answer["value"]) {
            let identity = &member["identity"];
            if identity["isContainer"] == true {
                continue;
            }
            let (Some(id), Some(name)) = (
                text(&identity["uniqueName"]),
                text(&identity["displayName"]),
            ) else {
                continue;
            };
            let found = |row: &PersonRow| row.id.eq_ignore_ascii_case(&id);
            let wanted = words.as_ref().is_none_or(|words| {
                format!("{} {}", name, id)
                    .to_lowercase()
                    .contains(words.as_str())
            });
            if wanted && !people.iter().any(found) {
                people.push(PersonRow {
                    id,
                    name,
                    team: team.to_owned(),
                });
            }
        }
    }
    if people.len() > args.limit {
        people.truncate(args.limit);
        ctx.note(format!("[first {}; --limit N for more]", args.limit));
    }
    Ok(people)
}

command! {
    pub PERSON_LIST = ["ado", "person", "list"], Read,
    "List a team's members with the address --assignee and @mentions take",
    keywords: ["people", "who", "members", "users", "email", "address", "colleague", "teammate", "assign", "mention"],
    example: "ado person list --text sam --fields id,name",
    run: person_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{BASE, ado, ado_with, page, urls};

    fn member(name: &str, address: &str) -> serde_json::Value {
        json!({"identity": {"id": format!("u-{name}"), "displayName": name, "uniqueName": address}})
    }

    #[test]
    fn members_of_every_configured_team_are_listed_once_and_filtered_by_text() {
        let config =
            "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\nteam = [\"Web\", \"Data\"]\n";
        let (outcome, transport) = ado_with(
            config,
            &["ado", "person", "list", "--text", "SAM"],
            vec![
                page(vec![
                    member("Sam Lee", "sam@contoso.com"),
                    member("Jane Doe", "jane@contoso.com"),
                    json!({"identity": {"displayName": "[Fabrikam]\\Web Admins",
                        "uniqueName": "vstfs:///Classification/TeamProject/x\\Web Admins", "isContainer": true}}),
                ]),
                page(vec![
                    member("Sam Lee", "SAM@contoso.com"),
                    member("Ana Samson", "ana@contoso.com"),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "sam@contoso.com", "name": "Sam Lee", "team": "Web"},
                {"id": "ana@contoso.com", "name": "Ana Samson", "team": "Data"}
            ])
        );
        assert_eq!(
            urls(&transport),
            [
                format!(
                    "{BASE}/_apis/projects/Fabrikam/teams/Web/members?$top=1000&api-version=7.1"
                ),
                format!(
                    "{BASE}/_apis/projects/Fabrikam/teams/Data/members?$top=1000&api-version=7.1"
                ),
            ]
        );
    }

    #[test]
    fn a_named_team_is_read_alone_and_the_limit_says_when_it_cut() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "person",
                "list",
                "--team",
                "Data Team",
                "--limit",
                "1",
            ],
            vec![page(vec![
                member("Sam Lee", "sam@contoso.com"),
                member("Jane Doe", "jane@contoso.com"),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json().as_array().unwrap().len(), 1);
        assert_eq!(outcome.stderr, "[first 1; --limit N for more]\n");
        assert!(urls(&transport)[0].contains("/teams/Data%20Team/members"));

        let config = "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\n";
        let (outcome, transport) = ado_with(config, &["ado", "person", "list"], vec![]);
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains("hint: agent-cli ado team list"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().is_empty());
    }
}
