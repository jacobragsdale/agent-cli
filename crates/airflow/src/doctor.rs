//! The overview's line for airflow, and `agent-cli doctor airflow`.

use agent_cli_core::{Check, Config, Ctx, Failure};

use crate::client::{Airflow, Client};

/// `airflow 3 instances`, from config alone.
pub(crate) fn status(config: &Config) -> String {
    if !config.has_section("airflow") {
        return "airflow not set up".to_owned();
    }
    match Airflow::load(config) {
        Ok(airflow) if airflow.instances.len() == 1 => "airflow 1 instance".to_owned(),
        Ok(airflow) => format!("airflow {} instances", airflow.instances.len()),
        Err(_) => "airflow config broken".to_owned(),
    }
}

/// Per instance: the server is Airflow 2.9+ or 3 and healthy (both need no
/// credential), then the credential signs in, saying how, and reads the
/// import errors.
pub(crate) fn doctor(ctx: &Ctx) -> Vec<Check> {
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
        let version = client.probe().and_then(|_| client.public("version"));
        let version = version
            .as_ref()
            .map(|v| v["version"].as_str().unwrap_or("?"));
        match version {
            Ok(version) => {
                let (major, minor) = version.split_once('.').unwrap_or((version, ""));
                let minor: u32 = minor.split('.').next().unwrap_or("").parse().unwrap_or(0);
                let detail = format!("Airflow {version} at {}", instance.base_url);
                checks.push(match major {
                    "3" => Check::ok(format!("{name} server"), detail),
                    "2" if minor >= 9 => Check::ok(format!("{name} server"), detail),
                    _ => Check::failed(
                        format!("{name} server"),
                        detail,
                        "agent-cli speaks Airflow 2.9+ (/api/v1) and 3 (/api/v2); older servers lack fields it reads",
                    ),
                });
            }
            Err(error) => {
                checks.push(Check::failed(
                    format!("{name} server"),
                    format!("{error:#}"),
                    "check base_url: the webserver's or API server's URL with any path prefix, without /api/v1 or /api/v2",
                ));
                continue;
            }
        }
        match client.health() {
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
                "the server answers its version but not its health",
            )),
        }
        match client.get("importErrors?limit=1") {
            Ok(errors) => {
                let count = errors["total_entries"].as_u64().unwrap_or_default();
                let by = client
                    .signed_in_by()
                    .map(|by| format!(" by {by}"))
                    .unwrap_or_default();
                let detail = format!(
                    "{} signs in{by}; {count} import error{}",
                    instance.auth_source(),
                    if count == 1 { "" } else { "s" }
                );
                checks.push(Check::ok(format!("{name} credential"), detail));
            }
            Err(error) => {
                let hint = error
                    .downcast_ref::<Failure>()
                    .and_then(|failure| failure.hint.clone())
                    .filter(|hint| !hint.contains("doctor airflow"))
                    .unwrap_or_else(|| {
                        format!(
                            "check the credential of [[airflow.instance]] {name:?} ({})",
                            instance.auth_source()
                        )
                    });
                checks.push(Check::failed(
                    format!("{name} credential"),
                    format!("{error:#}"),
                    hint,
                ));
            }
        }
    }
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, ctx, run};
    use serde_json::json;

    use super::*;
    use crate::DOMAIN;
    use crate::testing::{API, CONFIG, CONFIG_V1, paths};

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

        let old = FakeTransport::answering([
            Answer::json(&json!({"version": "2.7.3"})),
            Answer::json(&json!({"metadatabase": {"status": "healthy"}})),
            Answer::json(&json!({"import_errors": [], "total_entries": 0})),
        ]);
        let setup = Setup::fake(old.clone())
            .with_config(CONFIG_V1)
            .with_env("AIRFLOW_TOKEN", "t-1");
        let checks = doctor(&ctx(setup));
        assert!(
            !checks[0].ok && checks[0].hint.as_deref().unwrap().contains("Airflow 2.9+"),
            "{checks:?}"
        );
        assert_eq!(
            paths(&old),
            ["version", "health", "importErrors?limit=1"],
            "an older 2.x is named and still checked"
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

    /// An instance with a password and no `api`, so its version is asked.
    const PASSWORD: &str = "[[airflow.instance]]\nname = \"dev\"\nbase_url = \"http://localhost:18080\"\n\
                            username = \"admin\"\npassword_env = \"AIRFLOW_PASSWORD\"\n";

    /// Airflow 2.9 answering the probe, its version and its health.
    fn airflow_2() -> Vec<Answer> {
        vec![
            Answer::status(404, "<html>Not Found</html>"),
            Answer::json(&json!({"version": "2.9.3"})),
            Answer::json(&json!({"metadatabase": {"status": "healthy"}})),
        ]
    }

    /// Airflow 2's answer to a credential it refuses, and to a missing
    /// permission alike.
    fn forbidden() -> Answer {
        Answer::status(
            403,
            r#"{"detail": null, "status": 403, "title": "Forbidden"}"#,
        )
    }

    fn login_page() -> Answer {
        Answer::status(
            200,
            r#"<form method="post"><input id="username" name="username"><input id="password" name="password" type="password">
               <input id="csrf_token" name="csrf_token" type="hidden" value="csrf-1"></form>"#,
        )
        .with_header("Set-Cookie", "session=before-1; HttpOnly; Path=/; SameSite=Lax")
    }

    fn doctor_2(answers: Vec<Answer>) -> (Vec<Check>, FakeTransport) {
        let transport = FakeTransport::answering(answers);
        let setup = Setup::fake(transport.clone())
            .with_config(PASSWORD)
            .with_env("AIRFLOW_PASSWORD", "admin-pw-1");
        (doctor(&ctx(setup)), transport)
    }

    #[test]
    fn airflow_2_is_found_by_its_version_and_a_password_goes_as_http_basic() {
        let mut answers = airflow_2();
        answers.push(Answer::json(
            &json!({"import_errors": [], "total_entries": 0}),
        ));
        let (checks, transport) = doctor_2(answers);
        assert!(checks.iter().all(|check| check.ok), "{checks:?}");
        assert_eq!(
            checks[2].detail,
            "password for admin (password_env AIRFLOW_PASSWORD) signs in by HTTP Basic; 0 import errors"
        );
        let sent = transport.sent();
        let urls: Vec<&str> = sent.iter().map(|sent| sent.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "http://localhost:18080/api/v2/version",
                "http://localhost:18080/api/v1/version",
                "http://localhost:18080/api/v1/health",
                "http://localhost:18080/api/v1/importErrors?limit=1"
            ]
        );
        assert!(sent[..3].iter().all(|sent| sent.authorization.is_none()));
        let basic = format!("Basic {}", agent_cli_core::base64(b"admin:admin-pw-1"));
        assert_eq!(sent[3].authorization.as_deref(), Some(basic.as_str()));
    }

    #[test]
    fn a_session_only_airflow_2_signs_in_through_its_form_and_sends_the_cookie() {
        let mut answers = airflow_2();
        answers.extend([
            forbidden(),
            login_page(),
            Answer::status(302, "")
                .with_header("Location", "/home")
                .with_header("Set-Cookie", "session=signed-in-1; HttpOnly; Path=/"),
            Answer::json(&json!({"import_errors": [], "total_entries": 2})),
        ]);
        let (checks, transport) = doctor_2(answers);
        assert!(
            checks[2].ok && checks[2].detail.contains("signs in by the sign-in form"),
            "{checks:?}"
        );
        let sent = transport.sent();
        let cookie = |at: usize| {
            sent[at]
                .headers
                .iter()
                .find(|(name, _)| name == "Cookie")
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(sent[4].url, "http://localhost:18080/login/");
        assert!(sent[5].method.is_read(), "the sign-in changes nothing");
        assert_eq!(
            sent[5].body,
            Some(json!({"username": "admin", "password": "admin-pw-1", "csrf_token": "csrf-1"}))
        );
        assert_eq!(cookie(5), Some("session=before-1"));
        assert_eq!(cookie(6), Some("session=signed-in-1"));
        assert_eq!(sent[6].authorization, None, "the cookie alone");
    }

    #[test]
    fn a_wrong_password_is_exit_3_and_an_sso_user_is_told_to_ask_for_a_service_account() {
        let refused = || {
            vec![
                Answer::status(404, "<html>Not Found</html>"),
                forbidden(),
                login_page(),
                Answer::status(302, "").with_header("Location", "/login/?next=%2Fhome"),
            ]
        };
        let outcome = run(
            &[DOMAIN],
            &["airflow", "dag", "list"],
            Setup::fake(FakeTransport::answering(refused()))
                .with_config(PASSWORD)
                .with_env("AIRFLOW_PASSWORD", "wrong-pw-1"),
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("refused the password for admin, by its sign-in form and as HTTP Basic")
                && !outcome.stderr.contains("wrong-pw-1"),
            "{}",
            outcome.stderr
        );

        let mut answers = airflow_2();
        answers.extend([
            forbidden(),
            Answer::status(200, r#"<a href="/login/contoso-sso">Sign in with SSO</a>"#),
        ]);
        let (checks, _) = doctor_2(answers);
        assert!(
            !checks[2].ok
                && checks[2]
                    .hint
                    .as_deref()
                    .unwrap()
                    .contains("service account"),
            "{checks:?}"
        );
    }
}
