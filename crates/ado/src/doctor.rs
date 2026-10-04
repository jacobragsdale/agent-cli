//! The overview's line for ado, and `agent-cli doctor ado`.

use agent_cli_core::{Check, Config, Ctx};

use crate::client::{self, Ado};
use crate::config::{az_config_path, az_defaults};

/// `ado contoso/Fabrikam`, from config and the az devops defaults alone.
pub(crate) fn status(config: &Config) -> String {
    match Ado::names(config) {
        Ok(ado) => format!("ado {}/{}", ado.org, ado.projects.join(", ")),
        Err(_) if config.has_section("ado") => "ado config broken".to_owned(),
        Err(_) => "ado not set up".to_owned(),
    }
}

/// The config resolves; a credential is at hand; connection data answers;
/// the projects exist. Every call is bounded by the doctor's deadline.
pub(crate) fn doctor(ctx: &Ctx) -> Vec<Check> {
    let configured =
        ctx.config().has_section("ado") || az_defaults(az_config_path()) != (None, None);
    if !configured {
        return Vec::new();
    }
    let ado = match Ado::load(ctx) {
        Ok(ado) => ado,
        Err(error) => {
            return vec![Check::failed(
                "config",
                format!("{error:#}"),
                "set org and project under [ado]; `agent-cli config example ado` shows every key",
            )];
        }
    };
    let mut checks = vec![Check::ok(
        "config",
        format!(
            "{}/{}{}{}",
            ado.org,
            ado.projects.join(", "),
            if ado.code_project == ado.project {
                String::new()
            } else {
                format!(", code in {}", ado.code_project)
            },
            if ado.all_teams.is_empty() {
                ", no team (so no @current)".to_owned()
            } else {
                let teams: Vec<String> = (ado.all_teams.iter())
                    .map(|(project, team)| {
                        if *project == ado.project {
                            team.clone()
                        } else {
                            format!("{project}/{team}")
                        }
                    })
                    .collect();
                format!(", team {}", teams.join(", "))
            }
        ),
    )];
    let hint = "set pat_env or pat_cmd under [ado] (or AZURE_DEVOPS_EXT_PAT), or run `az login`";
    match ado.authorization(ctx, false) {
        Ok(_) => checks.push(Check::ok(
            "credential",
            ado.pat_source()
                .map_or("an az token for Azure DevOps".to_owned(), |source| {
                    format!("a personal access token from {source}")
                }),
        )),
        Err(error) => {
            checks.push(Check::failed("credential", format!("{error:#}"), hint));
            return checks;
        }
    }
    match ado.fetch_me(ctx) {
        Ok(me) => checks.push(Check::ok(
            "connection",
            format!("{} answers; signed in as {}", ado.org, me.name),
        )),
        Err(error) => {
            checks.push(Check::failed("connection", format!("{error:#}"), hint));
            return checks;
        }
    }
    let mut projects = ado.projects.clone();
    if !projects.contains(&ado.code_project) {
        projects.push(ado.code_project.clone());
    }
    for project in projects {
        let url = ado.api(
            None,
            &format!("projects/{}", client::segment(&project)),
            "",
            client::API,
        );
        checks.push(match ado.get(ctx, &url) {
            Ok(_) => Check::ok(format!("project {project}"), "exists"),
            Err(error) => Check::failed(
                format!("project {project}"),
                format!("{error:#}"),
                "check [ado] project and code_project",
            ),
        });
    }
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};
    use serde_json::json;

    use super::*;
    use crate::{DOMAIN, testing};

    #[test]
    fn the_overview_names_the_org_and_project() {
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(status(&config(testing::CONFIG)), "ado contoso/Fabrikam");
        assert_eq!(
            status(&config(
                "[ado]\norg = \"contoso\"\nproject = [\"Fabrikam\", \"Mobile\"]\n"
            )),
            "ado contoso/Fabrikam, Mobile"
        );
        assert_eq!(
            status(&config("[ado]\norg = \"contoso\"\n")),
            "ado config broken"
        );
        assert_eq!(status(&config("[ado]\nbogus = 1\n")), "ado config broken");
    }

    #[test]
    fn doctor_checks_the_credential_the_connection_and_the_projects() {
        let transport = FakeTransport::answering([
            Answer::json(
                &json!({"authenticatedUser": {"id": "u-1", "providerDisplayName": "Jane Doe"}}),
            ),
            Answer::json(&json!({"id": "p-1", "name": "Fabrikam"})),
            Answer::status(
                404,
                r#"{"message":"TF200016: The project does not exist: Code."}"#,
            ),
        ]);
        let setup = Setup::fake(transport.clone())
            .with_config(&format!(
                "{}code_project = \"Code\"\npat_env = \"CONTOSO_PAT\"\n",
                testing::CONFIG
            ))
            .with_env("CONTOSO_PAT", "fixture-pat");
        let outcome = run(&[DOMAIN], &["doctor", "ado"], setup);
        assert_eq!(outcome.code, 1, "one check failed: {outcome:?}");
        let rows = outcome.json();
        let checks: Vec<(&str, bool)> = rows
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["domain"] == "ado")
            .map(|row| (row["check"].as_str().unwrap(), row["ok"].as_bool().unwrap()))
            .collect();
        assert_eq!(
            checks,
            [
                ("config", true),
                ("credential", true),
                ("connection", true),
                ("project Fabrikam", true),
                ("project Code", false)
            ]
        );
        assert!(outcome.stdout.contains("signed in as Jane Doe"));
        assert!(
            outcome
                .stdout
                .contains("a personal access token from pat_env CONTOSO_PAT"),
            "{}",
            outcome.stdout
        );
        assert!(!outcome.stdout.contains("fixture-pat") && !outcome.stderr.contains("Basic"));
        assert!(
            transport.sent()[1]
                .url
                .ends_with("/_apis/projects/Fabrikam?api-version=7.1")
        );
    }
}
