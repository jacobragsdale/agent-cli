//! `[ado]`: the organization, its projects and teams, and where the
//! personal access token comes from.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use agent_cli_core::{Credential, Ctx, Failure};
use anyhow::Result;
use serde::{Deserialize, Deserializer};

use crate::client::Ado;

/// `[ado]` in config.toml.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Section {
    /// A slug (`contoso`) or a URL (`https://dev.azure.com/contoso`).
    org: Option<String>,
    /// Where the work items live: one project or a list, the first the
    /// default.
    #[serde(default, deserialize_with = "one_or_many")]
    project: Vec<String>,
    /// Where the repositories, pull requests and pipelines live, when a shop
    /// keeps its board and its code in different projects.
    code_project: Option<String>,
    /// The team (or teams) whose sprint `@current` means: `TEAM` in the
    /// first project, or `PROJECT/TEAM`.
    #[serde(default, deserialize_with = "one_or_many")]
    team: Vec<String>,
    /// Refused: a PAT opens the whole organization, and config files end up
    /// in dotfile repositories. A key only to say so.
    pat: Option<String>,
    pat_env: Option<String>,
    pat_cmd: Option<String>,
}

/// `team = "Web"` and `team = ["Web", "Data"]` both read, as do projects.
fn one_or_many<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    let teams = match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(one) => vec![one],
        OneOrMany::Many(many) => many,
    };
    Ok(teams
        .into_iter()
        .map(|team| team.trim().to_owned())
        .filter(|team| !team.is_empty())
        .collect())
}

impl Ado {
    /// `[ado]` (with `AGENT_CLI_ADO_*` applied by core), falling back to the
    /// `az devops configure` defaults for the organization and project.
    pub(crate) fn load(ctx: &Ctx) -> Result<Self> {
        let section: Section = ctx.section("ado")?;
        if section.pat.is_some() {
            return Err(Failure::setup(
                "[ado] pat holds the token itself, which is refused: a personal access token opens the whole organization",
            )
            .hint("name where it comes from: pat_env = \"VAR\" or pat_cmd = \"pass show NAME\"")
            .into());
        }
        let pat = Credential::from_keys(
            "pat",
            None,
            section.pat_env.clone(),
            section.pat_cmd.clone(),
        )
        .map_err(|why| Failure::setup(format!("[ado]: {why}")))?;
        let mut ado = Self::resolve(
            section,
            || az_defaults(az_config_path()),
            ctx.config().path(),
        )?;
        // The variable the Azure DevOps CLI extension reads too.
        let from_env = || {
            ctx.env("AZURE_DEVOPS_EXT_PAT")?;
            Credential::from_keys("pat", None, Some("AZURE_DEVOPS_EXT_PAT".into()), None)
                .ok()
                .flatten()
        };
        ado.pat = pat.or_else(from_env);
        Ok(ado)
    }

    /// [`Self::load`] in `--project`'s project when one is given.
    pub(crate) fn load_in(ctx: &Ctx, project: Option<&str>) -> Result<Self> {
        let ado = Self::load(ctx)?;
        Ok(match project {
            Some(project) => ado.in_project(project),
            None => ado,
        })
    }

    /// This organization in `project`: a configured one by its configured
    /// spelling, any other as given (one the organization lacks is exit 2
    /// at the first request).
    pub(crate) fn in_project(mut self, project: &str) -> Self {
        let project = project.trim();
        if project.is_empty() {
            return self;
        }
        self.project = (self.projects.iter())
            .find(|known| known.eq_ignore_ascii_case(project))
            .cloned()
            .unwrap_or_else(|| project.to_owned());
        self.teams = teams_in(&self.all_teams, &self.project);
        self
    }

    /// The WIQL condition for where a list looks: every configured project
    /// in one query, else (one project, or `--project`) the URL's.
    pub(crate) fn projects_condition(&self, narrowed: bool) -> String {
        match self.projects.as_slice() {
            [_, _, ..] if !narrowed => {
                let quoted: Vec<String> = (self.projects.iter())
                    .map(|project| format!("'{}'", project.replace('\'', "''")))
                    .collect();
                format!("[System.TeamProject] IN ({})", quoted.join(", "))
            }
            _ => "[System.TeamProject] = @project".to_owned(),
        }
    }

    /// ` --project NAME` when this is not the default project, for a hint
    /// naming a project-bound command.
    pub(crate) fn project_flag(&self) -> String {
        match self.projects.first() {
            Some(first) if first.eq_ignore_ascii_case(&self.project) => String::new(),
            _ => format!(" --project {}", crate::ids::arg(&self.project)),
        }
    }

    /// The names without a credential, for the overview's status line.
    pub(crate) fn names(config: &agent_cli_core::Config) -> Result<Self> {
        let section: Section = config.section("ado")?;
        Self::resolve(section, || az_defaults(az_config_path()), config.path())
    }

    pub(crate) fn resolve(
        section: Section,
        defaults: impl FnOnce() -> (Option<String>, Option<String>),
        path: &Path,
    ) -> Result<Self> {
        let blank =
            |value: Option<String>| value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let mut projects: Vec<String> = Vec::new();
        for project in section.project {
            if !projects.iter().any(|p| p.eq_ignore_ascii_case(&project)) {
                projects.push(project);
            }
        }
        let (mut org, mut project) = (blank(section.org), projects.first().cloned());
        if org.is_none() || project.is_none() {
            let (default_org, default_project) = defaults();
            org = org.or_else(|| blank(default_org));
            project = project.or_else(|| blank(default_project));
        }
        let missing = |key: &str, example: &str, az: &str| {
            Failure::setup(format!(
                "no Azure DevOps {key}: [ado] {key} is not set in {}",
                path.display()
            ))
            .hint(format!(
                "add `{key} = \"{example}\"` under [ado] (or set AGENT_CLI_ADO_{}), or run `az devops configure --defaults {az}`",
                key.to_uppercase()
            ))
        };
        let org = org.ok_or_else(|| {
            missing(
                "org",
                "contoso",
                "organization=https://dev.azure.com/contoso",
            )
        })?;
        let project = project.ok_or_else(|| missing("project", "Fabrikam", "project=Fabrikam"))?;
        if projects.is_empty() {
            projects.push(project.clone());
        }
        // Azure DevOps refuses `/` in a team's name, so the split is safe.
        let all_teams: Vec<(String, String)> = (section.team.iter())
            .map(|entry| match entry.split_once('/') {
                Some((named, team)) => {
                    let named = named.trim();
                    let known = projects.iter().find(|p| p.eq_ignore_ascii_case(named));
                    (
                        known.map_or(named, String::as_str).to_owned(),
                        team.trim().to_owned(),
                    )
                }
                None => (project.clone(), entry.clone()),
            })
            .collect();
        Ok(Self {
            org: crate::ids::org(&org)?,
            code_project: blank(section.code_project).unwrap_or_else(|| project.clone()),
            teams: teams_in(&all_teams, &project),
            all_teams,
            project,
            projects,
            pat: None,
            basic: OnceLock::new(),
        })
    }
}

/// `project`'s teams among `[ado] team`'s (project, team) pairs.
fn teams_in(all: &[(String, String)], project: &str) -> Vec<String> {
    (all.iter())
        .filter(|(named, _)| named.eq_ignore_ascii_case(project))
        .map(|(_, team)| team.clone())
        .collect()
}

/// The organization and project `az devops configure --defaults` saved.
pub(crate) fn az_defaults(path: Option<PathBuf>) -> (Option<String>, Option<String>) {
    let Some(raw) = path.and_then(|path| std::fs::read_to_string(path).ok()) else {
        return (None, None);
    };
    let (mut org, mut project) = (None, None);
    for line in raw.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "organization" => org = Some(value.trim().to_owned()),
            "project" => project = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    (org, project)
}

pub(crate) fn az_config_path() -> Option<PathBuf> {
    let dir = std::env::var_os("AZURE_CONFIG_DIR").map_or_else(
        || std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".azure")),
        |dir| Some(PathBuf::from(dir)),
    )?;
    Some(dir.join("azuredevops").join("config"))
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, FakeTransport, ctx, run};
    use agent_cli_core::{Config, Exit, Setup};
    use serde_json::json;

    use super::*;
    use crate::{DOMAIN, testing};

    fn section(toml: &str) -> Section {
        Config::parse("c.toml", Some(toml), Vec::new())
            .section("ado")
            .unwrap()
    }

    #[test]
    fn config_takes_urls_or_slugs_one_team_or_several_and_the_code_project_defaults() {
        let none = || (None, None);
        let ado = Ado::resolve(
            section("[ado]\norg = \"https://dev.azure.com/contoso/\"\nproject = \"Fabrikam\"\nteam = \"Web Team\"\n"),
            none,
            Path::new("c.toml"),
        )
        .unwrap();
        assert_eq!(
            (ado.org.as_str(), ado.project.as_str()),
            ("contoso", "Fabrikam")
        );
        assert_eq!(ado.code_project, "Fabrikam");
        assert_eq!(ado.teams, ["Web Team"]);

        let ado = Ado::resolve(
            section("[ado]\norg = \"https://contoso.visualstudio.com\"\nproject = \"Board\"\ncode_project = \"Code\"\nteam = [\"A\", \" \", \"B\"]\n"),
            none,
            Path::new("c.toml"),
        )
        .unwrap();
        assert_eq!(
            (ado.org.as_str(), ado.code_project.as_str()),
            ("contoso", "Code")
        );
        assert_eq!(ado.teams, ["A", "B"]);
    }

    #[test]
    fn a_missing_org_or_project_falls_back_to_az_defaults_then_is_needs_setup() {
        let defaults = || {
            (
                Some("https://dev.azure.com/fabrikam".to_owned()),
                Some("Web".to_owned()),
            )
        };
        let ado = Ado::resolve(Section::default(), defaults, Path::new("c.toml")).unwrap();
        assert_eq!(
            (ado.org.as_str(), ado.project.as_str()),
            ("fabrikam", "Web")
        );

        let error = Ado::resolve(
            section("[ado]\norg = \"contoso\"\n"),
            || (None, None),
            Path::new("/x/c.toml"),
        )
        .err()
        .unwrap();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.exit, Exit::Setup);
        assert_eq!(
            failure.message,
            "no Azure DevOps project: [ado] project is not set in /x/c.toml"
        );
        assert!(
            failure
                .hint
                .as_deref()
                .unwrap()
                .contains("AGENT_CLI_ADO_PROJECT")
        );

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        std::fs::write(
            &file,
            "[defaults]\norganization = https://dev.azure.com/contoso/\nproject = Fabrikam\n",
        )
        .unwrap();
        assert_eq!(
            az_defaults(Some(file)),
            (
                Some("https://dev.azure.com/contoso/".into()),
                Some("Fabrikam".into())
            )
        );
        assert_eq!(az_defaults(Some(dir.path().join("missing"))), (None, None));
    }

    #[test]
    fn projects_are_one_or_a_list_and_teams_may_name_their_project() {
        let ado = Ado::resolve(
            section("[ado]\norg = \"contoso\"\nproject = [\"Fabrikam\", \"Contoso Mobile\", \"fabrikam\"]\nteam = [\"Web Team\", \"contoso mobile/Apps\"]\n"),
            || (None, None),
            Path::new("c.toml"),
        )
        .unwrap();
        assert_eq!(ado.project, "Fabrikam", "the first is the default");
        assert_eq!(ado.projects, ["Fabrikam", "Contoso Mobile"]);
        assert_eq!(ado.code_project, "Fabrikam");
        assert_eq!(ado.teams, ["Web Team"]);
        assert_eq!(
            ado.projects_condition(false),
            "[System.TeamProject] IN ('Fabrikam', 'Contoso Mobile')"
        );
        assert_eq!(
            ado.projects_condition(true),
            "[System.TeamProject] = @project"
        );
        assert_eq!(ado.project_flag(), "");

        let mobile = ado.clone().in_project("CONTOSO MOBILE");
        assert_eq!(mobile.project, "Contoso Mobile", "its configured spelling");
        assert_eq!(mobile.teams, ["Apps"]);
        assert_eq!(mobile.project_flag(), " --project 'Contoso Mobile'");
        assert_eq!(
            mobile.work("wit/wiql", ""),
            "https://dev.azure.com/contoso/Contoso%20Mobile/_apis/wit/wiql?api-version=7.1"
        );
        let other = ado.in_project("Elsewhere");
        assert_eq!(
            (other.project.as_str(), other.teams.len()),
            ("Elsewhere", 0)
        );
    }

    #[test]
    fn a_pat_comes_from_pat_env_or_pat_cmd_before_the_environment_and_never_from_the_file() {
        let config = |extra: &str| format!("{}{extra}", testing::CONFIG);
        let transport = FakeTransport::answering([Answer::json(&json!({"id": 1, "rev": 1}))]);
        let setup = Setup::fake(transport.clone())
            .with_config(&config("pat_cmd = \"printf 'from-cmd\\\\n'\"\n"))
            .with_env("AZURE_DEVOPS_EXT_PAT", "fixture-pat");
        let outcome = run(
            &[DOMAIN],
            &["ado", "workitem", "get", "1", "--comments", "0"],
            setup,
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent()[0].authorization.as_deref(),
            Some(crate::client::basic("from-cmd").expose()),
            "pat_cmd wins over AZURE_DEVOPS_EXT_PAT"
        );

        let ctx = |extra: &str| {
            ctx(Setup::fake(FakeTransport::default())
                .with_config(&config(extra))
                .with_env("CONTOSO_PAT", "x"))
        };
        let source = |extra: &str| Ado::load(&ctx(extra)).unwrap().pat_source();
        assert_eq!(
            source("pat_env = \"CONTOSO_PAT\"\n").as_deref(),
            Some("pat_env CONTOSO_PAT")
        );
        assert_eq!(source(""), None, "no PAT anywhere: an az token");

        for (extra, said) in [
            ("pat = \"abc\"\n", "[ado] pat holds the token itself"),
            (
                "pat_env = \"A\"\npat_cmd = \"b\"\n",
                "give one of pat, pat_env and pat_cmd",
            ),
        ] {
            let error = Ado::load(&ctx(extra)).err().unwrap();
            let failure = error.downcast_ref::<Failure>().unwrap();
            assert_eq!(failure.exit, Exit::Setup);
            assert!(failure.message.contains(said), "{}", failure.message);
        }
    }
}
