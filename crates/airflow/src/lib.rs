//! The airflow domain: DAGs, runs, task instances, their logs and import
//! errors, live from Apache Airflow 3's REST API (`/api/v2`). Airflow 2's
//! `/api/v1` is out of scope: doctor names a v2 server and stops.
//!
//! Every id is the ref: a DAG is `etl_nightly`, a run `DAG/RUN`, a task
//! instance `DAG/RUN/TASK[:MAP][/TRY]`, and an instance with a `k8s_scope`
//! prints the pod a task ran in as the id `k8s pod logs` takes.

mod client;
mod dag;
mod run;
mod task;

use agent_cli_core::{Check, Config, Ctx, Domain, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Airflow, Client};

pub const DOMAIN: Domain = Domain {
    name: "airflow",
    summary: "Apache Airflow",
    commands: &[
        INSTANCE_LIST,
        dag::DAG_LIST,
        dag::DAG_GET,
        dag::DAG_UPDATE,
        run::RUN_LIST,
        run::RUN_GET,
        run::RUN_CREATE,
        run::RUN_WAIT,
        run::RUN_RETRY,
        task::TASK_LIST,
        task::TASK_GET,
        task::TASK_LOGS,
        task::TASK_RETRY,
        dag::IMPORT_ERROR_LIST,
        dag::IMPORT_ERROR_GET,
    ],
    // Airflow's own phrases only: search applies them to every domain.
    synonyms: &[
        ("dag run", &["run"]),
        ("dag runs", &["run"]),
        ("dagrun", &["run"]),
        ("dagruns", &["run"]),
        ("task instance", &["task"]),
        ("task instances", &["task"]),
        ("data pipeline", &["dag"]),
        ("broken dag", &["import", "error"]),
    ],
    status,
    doctor,
};

// ---------- airflow instance list ----------

#[derive(clap::Args)]
pub struct NoArgs {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InstanceRow {
    /// What --instance takes.
    name: String,
    base_url: String,
    /// The kind of credential and where it comes from, never its value.
    auth: String,
    /// Every change on it is refused.
    read_only: bool,
    /// The [[k8s.scope]] its task pods run in.
    k8s_scope: Option<String>,
    k8s_namespace: Option<String>,
}

fn instance_list(ctx: &Ctx, _: NoArgs) -> Result<Vec<InstanceRow>> {
    let airflow = Airflow::load(ctx.config())?;
    Ok(airflow
        .instances
        .iter()
        .map(|instance| InstanceRow {
            name: instance.name.clone(),
            base_url: instance.base_url.clone(),
            auth: instance.auth_source(),
            read_only: instance.read_only,
            k8s_scope: instance.k8s_scope.clone(),
            k8s_namespace: instance
                .k8s_scope
                .as_ref()
                .map(|_| instance.k8s_namespace.clone()),
        })
        .collect())
}

command! {
    pub INSTANCE_LIST = ["airflow", "instance", "list"], Read,
    "List the configured Airflow instances (from config, no network)",
    keywords: ["environment", "environments", "server", "deployment", "configured", "astro", "composer", "mwaa"],
    example: "airflow instance list --fields name,base_url,read_only",
    run: instance_list,
}

/// `airflow 3 instances`, from config alone.
fn status(config: &Config) -> String {
    if !config.has_section("airflow") {
        return "airflow not set up".to_owned();
    }
    match Airflow::load(config) {
        Ok(airflow) if airflow.instances.len() == 1 => "airflow 1 instance".to_owned(),
        Ok(airflow) => format!("airflow {} instances", airflow.instances.len()),
        Err(_) => "airflow config broken".to_owned(),
    }
}

/// Per instance: the server is Airflow 3 and healthy (both need no
/// credential), then the credential signs in and reads the import errors.
fn doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("airflow") {
        return Vec::new();
    }
    let airflow = match Airflow::load(ctx.config()) {
        Ok(airflow) => airflow,
        Err(error) => {
            return vec![Check::failed(
                "config",
                format!("{error:#}"),
                "fix [[airflow.instance]]; config.example.toml shows the keys",
            )];
        }
    };
    let mut checks = Vec::new();
    for instance in &airflow.instances {
        let client = Client::new(ctx, instance);
        let name = &instance.name;
        let version = client.public("version");
        let version = version
            .as_ref()
            .map(|v| v["version"].as_str().unwrap_or("?"));
        match version {
            Ok(version) if version.starts_with('3') => {
                checks.push(Check::ok(
                    format!("{name} server"),
                    format!("Airflow {version} at {}", instance.base_url),
                ));
            }
            Ok(version) => {
                checks.push(Check::failed(
                    format!("{name} server"),
                    format!("Airflow {version} at {}", instance.base_url),
                    "agent-cli supports Airflow 3 only (/api/v2)",
                ));
                continue;
            }
            Err(error) => {
                checks.push(Check::failed(
                    format!("{name} server"),
                    format!("{error:#}"),
                    "check base_url: the API server's URL with any path prefix, without /api/v2",
                ));
                continue;
            }
        }
        match client.public("monitor/health") {
            Ok(health) => {
                let parts: Vec<String> =
                    ["metadatabase", "scheduler", "triggerer", "dag_processor"]
                        .iter()
                        .filter_map(|part| {
                            health[*part]["status"]
                                .as_str()
                                .map(|status| format!("{part} {status}"))
                        })
                        .collect();
                let unhealthy = parts.iter().any(|part| !part.ends_with(" healthy"));
                let detail = parts.join(", ");
                checks.push(if unhealthy {
                    Check::failed(
                        format!("{name} health"),
                        detail,
                        "an unhealthy scheduler leaves runs queued; see the Airflow deployment",
                    )
                } else {
                    Check::ok(format!("{name} health"), detail)
                });
            }
            Err(error) => checks.push(Check::failed(
                format!("{name} health"),
                format!("{error:#}"),
                "the server answers /api/v2/version but not its health",
            )),
        }
        match client.get("importErrors?limit=1") {
            Ok(errors) => {
                let count = errors["total_entries"].as_u64().unwrap_or_default();
                let detail = format!(
                    "{} signs in; {count} import error{}",
                    instance.auth_source(),
                    if count == 1 { "" } else { "s" }
                );
                checks.push(Check::ok(format!("{name} credential"), detail));
            }
            Err(error) => checks.push(Check::failed(
                format!("{name} credential"),
                format!("{error:#}"),
                format!(
                    "check the credential of [[airflow.instance]] {name:?} ({})",
                    instance.auth_source()
                ),
            )),
        }
    }
    checks
}

#[cfg(test)]
pub(crate) mod testkit {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, Outcome, run};
    use serde_json::Value;

    pub(crate) const API: &str = "https://airflow.contoso.example/api/v2";
    /// One instance with a bearer token from the environment, so no sign-in
    /// call precedes the ones a test records.
    pub(crate) const CONFIG: &str = "[[airflow.instance]]\nname = \"prod\"\n\
        base_url = \"https://airflow.contoso.example\"\ntoken_env = \"AIRFLOW_TOKEN\"\n\
        k8s_scope = \"prod\"\nk8s_namespace = \"web\"\n";
    pub(crate) const TOKEN: &str = "fixture-token-1";

    pub(crate) fn airflow(argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
        airflow_with(CONFIG, argv, answers)
    }

    pub(crate) fn airflow_with(
        config: &str,
        argv: &[&str],
        answers: Vec<Answer>,
    ) -> (Outcome, FakeTransport) {
        let transport = FakeTransport::answering(answers);
        let setup = Setup::fake(transport.clone())
            .with_config(config)
            .with_env("AIRFLOW_TOKEN", TOKEN);
        (run(&[crate::DOMAIN], argv, setup), transport)
    }

    /// The paths sent, after `/api/v2/`, in order.
    pub(crate) fn paths(transport: &FakeTransport) -> Vec<String> {
        transport
            .sent()
            .into_iter()
            .map(|sent| {
                sent.url
                    .strip_prefix(&format!("{API}/"))
                    .unwrap_or(&sent.url)
                    .to_owned()
            })
            .collect()
    }

    /// `testing::assert_dry_run` over [`CONFIG`]: the reads run, the first
    /// change is planned and never sent.
    pub(crate) fn dry_run(argv: &[&str], answers: Vec<Answer>) -> (Vec<Value>, String) {
        let mut argv = argv.to_vec();
        argv.push("--dry-run");
        let (outcome, transport) = airflow(&argv, answers);
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
        (
            printed["would"].as_array().cloned().unwrap_or_default(),
            outcome.stderr,
        )
    }
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, FakeTransport, ctx, run};
    use agent_cli_core::{Setup, check_registry};
    use serde_json::json;

    use super::*;
    use crate::testkit::{API, CONFIG, airflow_with};

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(DOMAIN.commands.len(), 15);
    }

    #[test]
    fn read_only_mode_refuses_every_change_before_any_request() {
        agent_cli_core::testing::assert_read_only_refuses(&[DOMAIN]);
    }

    #[test]
    fn instance_list_shows_the_auth_kind_and_never_the_secret() {
        let config = format!(
            "{CONFIG}\n[[airflow.instance]]\nname = \"dev\"\nbase_url = \"http://localhost:8080/\"\n\
             username = \"agent\"\npassword = \"in-file-password-1\"\nread_only = true\n"
        );
        let (outcome, transport) = airflow_with(&config, &["airflow", "instance", "list"], vec![]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"name": "prod", "base_url": "https://airflow.contoso.example",
                 "auth": "token (token_env AIRFLOW_TOKEN)", "read_only": false,
                 "k8s_scope": "prod", "k8s_namespace": "web"},
                {"name": "dev", "base_url": "http://localhost:8080",
                 "auth": "password for agent (password (in the config file))", "read_only": true}
            ])
        );
        assert!(!outcome.stdout.contains("in-file-password-1"));
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn a_broken_instance_is_exit_3_naming_it() {
        for (body, want) in [
            (
                "base_url = \"http://airflow.contoso.example\"\ntoken_env = \"T\"",
                "plain http only to localhost",
            ),
            (
                "base_url = \"https://airflow.contoso.example/api/v2\"\ntoken_env = \"T\"",
                "ends in /api/v2",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"\nusername = \"a\"",
                "username needs",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"",
                "no credential",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"\ntoken_env = \"T\"\ntoken_cmd = \"x\"",
                "give one of token",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"\ntoken_env = \"T\"\nbogus = 1",
                "unknown field",
            ),
        ] {
            let config = format!("[[airflow.instance]]\nname = \"prod\"\n{body}\n");
            let (outcome, _) = airflow_with(&config, &["airflow", "dag", "list"], vec![]);
            assert_eq!(outcome.code, 3, "{body}: {outcome:?}");
            assert!(outcome.stderr.contains(want), "{body}: {}", outcome.stderr);
        }
    }

    #[test]
    fn a_password_signs_in_once_and_again_after_a_401_and_the_jwt_never_prints() {
        let config = "[[airflow.instance]]\nname = \"dev\"\nbase_url = \"http://localhost:8080\"\n\
                      username = \"agent\"\npassword_env = \"AIRFLOW_PASSWORD\"\n";
        let page = json!({"dags": [], "total_entries": 0});
        let transport = FakeTransport::answering([
            Answer::status(201, r#"{"access_token":"eyJhbGciOi.first-jwt.sig1"}"#),
            Answer::status(401, r#"{"detail":"Token expired"}"#),
            Answer::status(201, r#"{"access_token":"eyJhbGciOi.second-jwt.sig2"}"#),
            Answer::json(&page),
        ]);
        let setup = Setup::fake(transport.clone())
            .with_config(config)
            .with_env("AIRFLOW_PASSWORD", "dev-password-1");
        let outcome = run(&[DOMAIN], &["airflow", "dag", "list"], setup);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        let calls: Vec<(&str, &str, Option<&str>)> = sent
            .iter()
            .map(|s| (s.method.wire(), s.url.as_str(), s.authorization.as_deref()))
            .collect();
        let dags = "http://localhost:8080/api/v2/dags?order_by=dag_id&limit=50&offset=0";
        assert_eq!(
            calls,
            [
                ("POST", "http://localhost:8080/auth/token", None),
                ("GET", dags, Some("Bearer eyJhbGciOi.first-jwt.sig1")),
                ("POST", "http://localhost:8080/auth/token", None),
                ("GET", dags, Some("Bearer eyJhbGciOi.second-jwt.sig2")),
            ]
        );
        assert!(sent[0].method.is_read(), "the sign-in is a read");
        assert_eq!(
            sent[0].body,
            Some(json!({"username": "agent", "password": "dev-password-1"}))
        );
        for secret in ["first-jwt", "second-jwt", "dev-password-1"] {
            assert!(!outcome.stdout.contains(secret) && !outcome.stderr.contains(secret));
        }

        let refused =
            FakeTransport::answering([Answer::status(401, r#"{"detail":"Invalid credentials"}"#)]);
        let setup = Setup::fake(refused)
            .with_config(config)
            .with_env("AIRFLOW_PASSWORD", "wrong-password-1");
        let outcome = run(&[DOMAIN], &["airflow", "dag", "list"], setup);
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Invalid credentials")
                && outcome
                    .stderr
                    .contains("`agent-cli doctor airflow` checks it"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn refusals_read_as_the_next_step() {
        for (answer, code, hint) in [
            (
                Answer::status(401, r#"{"detail":"Invalid token"}"#),
                3,
                "`agent-cli doctor airflow` checks it",
            ),
            (
                Answer::status(403, r#"{"detail":"Forbidden"}"#),
                1,
                "lacks this permission",
            ),
            (
                Answer::status(307, "").with_header("Location", "https://login.contoso.example/"),
                3,
                "base_url is wrong",
            ),
        ] {
            // A 401 is retried once with a fresh token, so it is answered twice.
            let answers = if answer.status == 401 {
                vec![answer.clone(), answer]
            } else {
                vec![answer]
            };
            let (outcome, _) = airflow_with(CONFIG, &["airflow", "dag", "list"], answers);
            assert_eq!(outcome.code, code, "{outcome:?}");
            assert!(outcome.stderr.contains(hint), "{}", outcome.stderr);
        }
    }

    #[test]
    fn two_instances_and_no_flag_is_exit_2_naming_both_before_any_request() {
        let config = format!(
            "{CONFIG}\n[[airflow.instance]]\nname = \"qa\"\nbase_url = \"https://qa.contoso.example\"\ntoken_env = \"T\"\n"
        );
        let (outcome, transport) = airflow_with(&config, &["airflow", "dag", "list"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("prod, qa"), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn the_overview_counts_the_instances() {
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(status(&config(CONFIG)), "airflow 1 instance");
        assert_eq!(status(&config("[airflow]\n")), "airflow 0 instances");
        assert_eq!(
            status(&config("[[airflow.instance]]\nname = \"x\"\n")),
            "airflow config broken"
        );
        assert_eq!(status(&config("")), "airflow not set up");
    }

    #[test]
    fn doctor_checks_the_version_the_health_and_the_credential() {
        let transport = FakeTransport::answering([
            Answer::json(&json!({"version": "3.3.2", "git_version": null})),
            Answer::json(&json!({
                "metadatabase": {"status": "healthy"},
                "scheduler": {"status": "healthy", "latest_scheduler_heartbeat": "2026-09-29T11:59:58Z"},
                "triggerer": {"status": "healthy"},
                "dag_processor": {"status": "unhealthy"}
            })),
            Answer::json(&json!({"import_errors": [], "total_entries": 1})),
        ]);
        let setup = Setup::fake(transport.clone())
            .with_config(CONFIG)
            .with_env("AIRFLOW_TOKEN", "doctor-token-1");
        let checks = doctor(&ctx(setup));
        let rows: Vec<(&str, bool)> = checks.iter().map(|c| (c.check.as_str(), c.ok)).collect();
        assert_eq!(
            rows,
            [
                ("prod server", true),
                ("prod health", false),
                ("prod credential", true)
            ]
        );
        assert!(
            checks[2].detail.ends_with("signs in; 1 import error"),
            "{checks:?}"
        );
        let sent: Vec<String> = transport.sent().into_iter().map(|s| s.url).collect();
        assert_eq!(
            sent,
            [
                format!("{API}/version"),
                format!("{API}/monitor/health"),
                format!("{API}/importErrors?limit=1")
            ]
        );

        let old = FakeTransport::answering([Answer::json(&json!({"version": "2.11.0"}))]);
        let setup = Setup::fake(old)
            .with_config(CONFIG)
            .with_env("AIRFLOW_TOKEN", "t-1");
        let checks = doctor(&ctx(setup));
        assert_eq!(checks.len(), 1);
        assert!(
            !checks[0].ok
                && checks[0]
                    .hint
                    .as_deref()
                    .unwrap()
                    .contains("Airflow 3 only")
        );

        let outcome = run(
            &[DOMAIN],
            &["doctor", "airflow"],
            Setup::fake(FakeTransport::default())
                .with_config("[[airflow.instance]]\nname = \"x\"\n"),
        );
        assert!(
            outcome.stdout.contains("\"check\":\"config\""),
            "{outcome:?}"
        );
    }
}
