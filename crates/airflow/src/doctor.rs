//! The overview's line for airflow, and `agent-cli doctor airflow`.

use agent_cli_core::{Check, Config, Ctx};

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

/// Per instance: the server is Airflow 3 and healthy (both need no
/// credential), then the credential signs in and reads the import errors.
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
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, ctx, run};
    use serde_json::json;

    use super::*;
    use crate::DOMAIN;
    use crate::testing::{API, CONFIG};

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
