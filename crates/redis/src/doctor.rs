//! The overview's line for redis, and `agent-cli doctor redis`.

use std::time::{Duration, Instant};

use agent_cli_core::{Check, Config, Ctx};

use crate::config::Redis;
use crate::session::{OnInstance, Session, cmd};

pub(crate) fn status(config: &Config) -> String {
    if !config.has_section("redis") {
        return "redis not set up".to_owned();
    }
    match Redis::load(config) {
        Ok(redis) if redis.instances.len() == 1 => "redis 1 instance".to_owned(),
        Ok(redis) => format!("redis {} instances", redis.instances.len()),
        Err(_) => "redis config broken".to_owned(),
    }
}

/// How long doctor gives one instance.
const PROBE: Duration = Duration::from_secs(5);

/// The section parses, and each instance signs in and answers `PING`
/// within [`PROBE`].
pub(crate) fn doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("redis") {
        return Vec::new();
    }
    let redis = match Redis::load(ctx.config()) {
        Ok(redis) => redis,
        Err(error) => {
            return vec![Check::failed(
                "config",
                format!("{error:#}"),
                "fix [redis]; `agent-cli config example redis` shows every key",
            )];
        }
    };
    let mut checks = vec![Check::ok(
        "config",
        match redis.instances.len() {
            1 => "1 instance".to_owned(),
            count => format!("{count} instances"),
        },
    )];
    for spec in &redis.instances {
        let check = format!("instance {}", spec.name);
        let started = Instant::now();
        let op = OnInstance {
            spec,
            deadline: ctx.deadline().min(started + PROBE),
            plan: serde_json::json!({"instance": spec.name, "read": "PING"}),
            work: |session: &mut Session| session.call(cmd!("PING")),
        };
        checks.push(match ctx.read(op) {
            Ok(_) => Check::ok(
                check,
                format!(
                    "{}:{}{} answered PING in {} ms, signed in with {}",
                    spec.host,
                    spec.port,
                    if spec.tls { " (TLS)" } else { "" },
                    started.elapsed().as_millis(),
                    spec.source()
                ),
            ),
            Err(error) => Check::failed(
                check,
                format!("{error:#}"),
                if spec.az {
                    format!(
                        "az login, then check that your Entra ID account has a data access policy on {:?}'s cache",
                        spec.name
                    )
                } else {
                    format!(
                        "check host, port, tls, user and password of {:?} in [redis]",
                        spec.name
                    )
                },
            ),
        });
    }
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{FakeTransport, ctx};

    use super::*;
    use crate::testing::{config, serve};

    #[test]
    fn the_overview_counts_instances_and_doctor_pings_each() {
        let parse = |toml: Option<&str>| Config::parse("c.toml", toml, Vec::new());
        assert_eq!(status(&parse(None)), "redis not set up");
        assert_eq!(
            status(&parse(Some("[redis]\nbogus = 1\n"))),
            "redis config broken"
        );
        let fake = serve(vec![("PING", crate::session::Reply::Status("PONG".into()))]);
        let toml = config(&[("orders-qa", fake.port), ("orders-prod", 9)]);
        assert_eq!(status(&parse(Some(&toml))), "redis 2 instances");
        let checks = doctor(&ctx(
            Setup::fake(FakeTransport::default()).with_config(&toml)
        ));
        let oks: Vec<(String, bool)> = checks
            .iter()
            .map(|check| (check.check.clone(), check.ok))
            .collect();
        assert_eq!(
            oks,
            [
                ("config".to_owned(), true),
                ("instance orders-qa".to_owned(), true),
                ("instance orders-prod".to_owned(), false)
            ]
        );
        assert!(
            checks[1].detail.contains("answered PING") && checks[1].detail.contains("no password"),
            "{:?}",
            checks[1]
        );
    }
}
