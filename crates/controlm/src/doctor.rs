//! The overview's line for controlm, and `agent-cli doctor controlm`.

use agent_cli_core::{Check, Config, Ctx, Failure};

use crate::client::{Client, ControlM, text};

/// `controlm 2 instances`, from config alone.
pub(crate) fn status(config: &Config) -> String {
    if !config.has_section("controlm") {
        return "controlm not set up".to_owned();
    }
    match ControlM::load(config) {
        Ok(controlm) if controlm.instances.len() == 1 => "controlm 1 instance".to_owned(),
        Ok(controlm) => format!("controlm {} instances", controlm.instances.len()),
        Err(_) => "controlm config broken".to_owned(),
    }
}

/// Per instance, one live call: the Control-M/Servers the credential sees,
/// which proves the endpoint, the certificate and the credential at once.
// VERIFY(work): that a user who is not an admin may read config/servers; if
// not, read `run/jobs/status?limit=1` instead. And the fields of each server
// (the spec: name, host, state, version).
pub(crate) fn doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("controlm") {
        return Vec::new();
    }
    let controlm = match ControlM::load(ctx.config()) {
        Ok(controlm) => controlm,
        Err(error) => {
            return vec![Check::failed(
                "config",
                format!("{error:#}"),
                "fix [[controlm.instance]]; `agent-cli config example controlm` shows the keys",
            )];
        }
    };
    let mut checks = Vec::new();
    for instance in &controlm.instances {
        let client = Client::new(ctx, instance);
        let name = &instance.name;
        match client.get("config/servers") {
            Ok(servers) => {
                let servers: Vec<String> = servers
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|server| {
                        let name = text(&server["name"])?;
                        Some(match text(&server["state"]) {
                            Some(state) => format!("{name} {state}"),
                            None => name,
                        })
                    })
                    .collect();
                let version = client
                    .version
                    .borrow()
                    .as_ref()
                    .map(|version| format!(" to Enterprise Manager {version}"))
                    .unwrap_or_default();
                checks.push(Check::ok(
                    format!("{name} credential"),
                    format!(
                        "{} signs in{version} at {}; servers: {}",
                        instance.auth_source(),
                        instance.base_url,
                        servers.join(", ")
                    ),
                ));
            }
            Err(error) => {
                let hint = error
                    .downcast_ref::<Failure>()
                    .and_then(|failure| failure.hint.clone())
                    .filter(|hint| !hint.contains("doctor controlm"))
                    .unwrap_or_else(|| {
                        format!(
                            "check base_url and the credential of [[controlm.instance]] {name:?} ({})",
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
    use agent_cli_core::testing::{Answer, run};
    use agent_cli_core::{Setup, testing::FakeTransport};
    use serde_json::json;

    use crate::testing::{CONFIG, TOKEN};

    #[test]
    fn doctor_names_the_servers_the_credential_sees() {
        let transport = FakeTransport::answering([Answer::json(&json!([
            {"name": "ctm-prod", "host": "ctm-prod.contoso.example", "state": "Up", "version": "9.0.21.200"}
        ]))]);
        let setup = Setup::fake(transport.clone())
            .with_config(CONFIG)
            .with_env("CONTROLM_TOKEN", TOKEN);
        let outcome = run(&[crate::DOMAIN], &["doctor", "controlm"], setup);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome.stdout.contains("servers: ctm-prod Up"),
            "{}",
            outcome.stdout
        );
        assert!(!outcome.stdout.contains(TOKEN));
    }
}
