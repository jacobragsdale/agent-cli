//! The ado domain: work items, pull requests, pipelines, runs and approvals,
//! live from Azure DevOps. Ported from ticket-tui without its SQLite cache:
//! every read asks Azure DevOps (WIQL and REST), so an answer is never older
//! than the command.

mod client;
mod markdown;
mod pipeline;
mod pr;
mod workitem;

use agent_cli_core::{Check, Config, Ctx, Domain};

use crate::client::{Ado, az_config_path, az_defaults};

pub const DOMAIN: Domain = Domain {
    name: "ado",
    summary: "Azure DevOps",
    commands: &[
        workitem::WORKITEM_LIST,
        workitem::WORKITEM_GET,
        workitem::WORKITEM_CREATE,
        workitem::WORKITEM_UPDATE,
        workitem::WORKITEM_COMMENT,
        workitem::WORKITEM_LINK,
        workitem::TEAM_LIST,
        pr::REPO_LIST,
        pr::REPO_GET,
        pr::PR_LIST,
        pr::PR_GET,
        pr::PR_CREATE,
        pr::PR_VOTE,
        pr::PR_UPDATE,
        pr::PR_LINK,
        pr::PR_COMMENT,
        pr::PR_COMPLETE,
        pr::PR_ABANDON,
        pipeline::PIPELINE_LIST,
        pipeline::RUN_LIST,
        pipeline::RUN_GET,
        pipeline::RUN_LOGS,
        pipeline::RUN_CREATE,
        pipeline::RUN_WAIT,
        pipeline::RUN_CANCEL,
        pipeline::RUN_RETRY,
        pipeline::APPROVAL_LIST,
        pipeline::APPROVAL_APPROVE,
        pipeline::APPROVAL_REJECT,
    ],
    synonyms: &[
        ("ticket", &["workitem"]),
        ("tickets", &["workitem"]),
        ("bug", &["workitem"]),
        ("bugs", &["workitem"]),
        ("story", &["workitem"]),
        ("stories", &["workitem"]),
        ("user story", &["workitem"]),
        ("task", &["workitem"]),
        ("tasks", &["workitem"]),
        ("issue", &["workitem"]),
        ("issues", &["workitem"]),
        ("backlog", &["workitem"]),
        ("work item", &["workitem"]),
        ("work items", &["workitem"]),
        ("pull request", &["pr"]),
        ("pull requests", &["pr"]),
        ("merge request", &["pr"]),
        ("review", &["pr"]),
        ("build", &["run", "pipeline"]),
        ("builds", &["run", "pipeline"]),
        ("ci", &["run", "pipeline"]),
        ("sprint", &["iteration"]),
        ("gate", &["approval"]),
        ("repository", &["repo"]),
        ("repositories", &["repo"]),
        ("azure devops", &["ado"]),
        ("devops", &["ado"]),
    ],
    status,
    doctor,
};

/// `ado contoso/Fabrikam`, from config and the az devops defaults alone.
fn status(config: &Config) -> String {
    match Ado::names(config) {
        Ok(ado) => format!("ado {}/{}", ado.org, ado.project),
        Err(_) if config.has_section("ado") => "ado config broken".to_owned(),
        Err(_) => "ado not set up".to_owned(),
    }
}

/// The config resolves; a credential is at hand; connection data answers;
/// the projects exist. Every call is bounded by the doctor's deadline.
fn doctor(ctx: &Ctx) -> Vec<Check> {
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
                "set org and project under [ado]; config.example.toml shows every key",
            )];
        }
    };
    let mut checks = vec![Check::ok(
        "config",
        format!(
            "{}/{}{}{}",
            ado.org,
            ado.project,
            if ado.code_project == ado.project {
                String::new()
            } else {
                format!(", code in {}", ado.code_project)
            },
            if ado.teams.is_empty() {
                ", no team (so no @current)".to_owned()
            } else {
                format!(", team {}", ado.teams.join(", "))
            }
        ),
    )];
    let hint = "run `az login`, or set AZURE_DEVOPS_EXT_PAT";
    match ado.authorization(ctx, false) {
        Ok(_) if ado.uses_pat() => checks.push(Check::ok("credential", "AZURE_DEVOPS_EXT_PAT")),
        Ok(_) => checks.push(Check::ok("credential", "an az token for Azure DevOps")),
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
    let mut projects = vec![ado.project.clone()];
    if ado.code_project != ado.project {
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
pub(crate) mod testkit {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, Outcome, run};
    use serde_json::Value;

    pub(crate) const CONFIG: &str =
        "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\nteam = \"Web Team\"\n";

    /// Runs `argv` over `answers` with [`CONFIG`]; the transport shows what
    /// was sent.
    pub(crate) fn ado(argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
        ado_with(CONFIG, argv, answers)
    }

    pub(crate) fn ado_with(
        config: &str,
        argv: &[&str],
        answers: Vec<Answer>,
    ) -> (Outcome, FakeTransport) {
        let transport = FakeTransport::answering(answers);
        let setup = Setup::fake(transport.clone()).with_config(config);
        (run(&[crate::DOMAIN], argv, setup), transport)
    }

    /// The URLs sent, in order.
    pub(crate) fn urls(transport: &FakeTransport) -> Vec<String> {
        transport.sent().into_iter().map(|sent| sent.url).collect()
    }

    /// `testing::assert_dry_run` for commands that read `[ado]` first: the
    /// reads run, the first change is planned and never sent.
    pub(crate) fn dry_run(argv: &[&str], answers: Vec<Answer>) -> Vec<Value> {
        let mut argv = argv.to_vec();
        argv.push("--dry-run");
        let (outcome, transport) = ado(&argv, answers);
        let writes: Vec<_> = transport
            .sent()
            .into_iter()
            .filter(|sent| !sent.method.is_read())
            .collect();
        assert!(
            writes.is_empty(),
            "a change was sent under --dry-run: {writes:?}"
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let printed = outcome.json();
        assert_eq!(
            printed["dry_run"], true,
            "never reached ctx.write: {outcome:?}"
        );
        assert_eq!(transport.remaining(), 0, "answers left over: {outcome:?}");
        printed["would"].as_array().cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, FakeTransport, run};
    use agent_cli_core::{Setup, check_registry};
    use serde_json::json;

    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(DOMAIN.commands.len(), 29);
    }

    #[test]
    fn read_only_mode_refuses_every_change_before_any_request() {
        agent_cli_core::testing::assert_read_only_refuses(&[DOMAIN]);
    }

    #[test]
    fn a_personal_access_token_in_the_environment_goes_as_basic() {
        let transport = FakeTransport::answering([Answer::json(&json!({"id": 1, "rev": 1}))]);
        let setup = Setup::fake(transport.clone())
            .with_config(testkit::CONFIG)
            .with_env("AZURE_DEVOPS_EXT_PAT", "fixture-pat");
        let outcome = run(
            &[DOMAIN],
            &["ado", "workitem", "get", "AB#1", "--comments", "0"],
            setup,
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent()[0].authorization.as_deref(),
            Some("Basic OmZpeHR1cmUtcGF0")
        );
    }

    #[test]
    fn the_overview_names_the_org_and_project() {
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(status(&config(testkit::CONFIG)), "ado contoso/Fabrikam");
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
            .with_config(&format!("{}code_project = \"Code\"\n", testkit::CONFIG));
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
        assert!(!outcome.stdout.contains("fixture-pat") && !outcome.stderr.contains("Basic"));
        assert!(
            transport.sent()[1]
                .url
                .ends_with("/_apis/projects/Fabrikam?api-version=7.1")
        );
    }
}
