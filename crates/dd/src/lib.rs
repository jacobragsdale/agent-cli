//! The dd domain: Datadog logs, metrics, monitors, downtimes, APM, incidents,
//! SLOs and dashboards, over Datadog's public REST API.
//!
//! It covers the calls agents make most while debugging and operating, with
//! rows that bound their size and point back at Kubernetes (`pod` is the k8s
//! pod id). Datadog's own CLI, pup, covers the long tail; `doctor dd` says
//! so. pup is not wrapped: see docs/plans/datadog.md.

mod apm;
mod catalog;
mod client;
mod logs;
mod metric;
mod monitor;

use agent_cli_core::{Check, Config, Ctx, Domain, Method};

use crate::client::{Dd, Section, site};

pub const DOMAIN: Domain = Domain {
    name: "dd",
    summary: "Datadog",
    commands: &[
        logs::LOG_LIST,
        logs::LOG_COUNT_LIST,
        metric::METRIC_LIST,
        metric::METRIC_GET,
        monitor::MONITOR_LIST,
        monitor::MONITOR_GET,
        monitor::DOWNTIME_LIST,
        monitor::DOWNTIME_CREATE,
        monitor::DOWNTIME_CANCEL,
        logs::EVENT_LIST,
        apm::SERVICE_LIST,
        apm::SERVICE_GET,
        apm::SPAN_LIST,
        catalog::INCIDENT_LIST,
        catalog::INCIDENT_GET,
        catalog::HOST_LIST,
        catalog::CONTAINER_LIST,
        catalog::SLO_LIST,
        catalog::SLO_GET,
        catalog::DASHBOARD_LIST,
    ],
    synonyms: &[
        ("datadog", &["dd"]),
        ("apm", &["span", "service"]),
        ("trace", &["span"]),
        ("traces", &["span"]),
        ("tracing", &["span"]),
        ("request", &["span"]),
        ("requests", &["span"]),
        ("mute", &["downtime", "create"]),
        ("silence", &["downtime", "create"]),
        ("snooze", &["downtime", "create"]),
        ("unmute", &["downtime", "cancel"]),
        ("maintenance", &["downtime"]),
        ("alert", &["monitor"]),
        ("alerts", &["monitor"]),
        ("alerting", &["monitor"]),
        ("alarm", &["monitor"]),
        ("alarms", &["monitor"]),
        ("outage", &["incident"]),
        ("sev", &["incident"]),
        ("timeseries", &["metric"]),
        ("graph", &["metric"]),
        ("error rate", &["service", "error_rate"]),
        ("latency", &["service", "p95_ms"]),
        ("dashboards", &["dashboard"]),
    ],
    status,
    doctor,
};

/// `dd eu`, from config alone: the site's short name.
fn status(config: &Config) -> String {
    if !config.has_section("datadog") && !config.has_section("dd") {
        return "dd not set up".to_owned();
    }
    match Section::load(config).and_then(|section| site(section.site.as_deref())) {
        Ok(site) => format!("dd {}", site.label),
        Err(_) => "dd config broken".to_owned(),
    }
}

/// The site and where the credential comes from (never its value), then one
/// live call: `validate` for a key pair, a one-row monitor search for a
/// token. pup, when it is on PATH, covers what dd does not.
fn doctor(ctx: &Ctx) -> Vec<Check> {
    let configured = ctx.config().has_section("datadog")
        || ctx.config().has_section("dd")
        || ctx.env("DD_ACCESS_TOKEN").is_some()
        || ctx.env("DD_API_KEY").is_some();
    if !configured {
        return Vec::new();
    }
    let dd = match Dd::load(ctx) {
        Ok(dd) => dd,
        Err(error) => {
            return vec![Check::failed(
                "config",
                format!("{error:#}"),
                "fix [datadog]; config.example.toml shows every key",
            )];
        }
    };
    let mut checks = vec![Check::ok(
        "config",
        format!("site {} ({})", dd.site.name, dd.site.app),
    )];
    let Some(source) = dd.source() else {
        checks.push(Check::failed(
            "credential",
            "none found",
            "export DD_ACCESS_TOKEN, or set token_cmd = \"pup auth token\" under [datadog]",
        ));
        return checks;
    };
    checks.push(Check::ok("credential", source));
    let (path, query) = if dd.uses_keys() {
        ("/api/v1/validate", Vec::new())
    } else {
        ("/api/v1/monitor/search", vec![("per_page", "1".to_owned())])
    };
    checks.push(
        match dd.send(ctx, None, Method::Get, dd.url(path, &query), None) {
            Ok(_) => Check::ok("connection", format!("api.{} answers", dd.site.name)),
            Err(error) => Check::failed(
                "connection",
                format!("{error:#}"),
                "a 403 names the scope the credential lacks: grant it, or use a token that has it",
            ),
        },
    );
    let mut pup = std::process::Command::new("pup");
    pup.arg("--version");
    if let Ok(output) = ctx.read(pup)
        && output.status.success()
    {
        checks.push(Check::ok(
            "pup",
            format!(
                "{} covers what dd does not; run it with PUP_READ_ONLY=1",
                output.stdout.trim()
            ),
        ));
    }
    checks
}

#[cfg(test)]
pub(crate) mod testkit {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, Outcome, run};
    use serde_json::Value;

    pub(crate) const CONFIG: &str =
        "[datadog]\nsite = \"datadoghq.eu\"\nenv = \"prod\"\ntoken_env = \"DD_TEST_TOKEN\"\n";
    pub(crate) const TOKEN: &str = "ddpat-fixture-token-7f3a91";

    /// Runs `argv` over `answers` with [`CONFIG`] and a token in the
    /// environment; the transport shows what was sent.
    pub(crate) fn dd(argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
        dd_with(CONFIG, argv, answers)
    }

    pub(crate) fn dd_with(
        config: &str,
        argv: &[&str],
        answers: Vec<Answer>,
    ) -> (Outcome, FakeTransport) {
        let transport = FakeTransport::answering(answers);
        let setup = Setup::fake(transport.clone())
            .with_config(config)
            .with_env("DD_TEST_TOKEN", TOKEN);
        let outcome = run(&[crate::DOMAIN], argv, setup);
        assert!(
            !outcome.stdout.contains(TOKEN) && !outcome.stderr.contains(TOKEN),
            "the token leaked: {outcome:?}"
        );
        (outcome, transport)
    }

    pub(crate) fn sent_bodies(transport: &FakeTransport) -> Vec<Value> {
        transport
            .sent()
            .into_iter()
            .map(|sent| sent.body.unwrap_or(Value::Null))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use agent_cli_core::check_registry;
    use agent_cli_core::testing::{Answer, FakeTransport, assert_read_only_refuses, run};
    use agent_cli_core::{Setup, Transport};
    use serde_json::json;

    use super::testkit::{CONFIG, TOKEN, dd, dd_with};
    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(DOMAIN.commands.len(), 20);
    }

    #[test]
    fn read_only_mode_refuses_every_change_before_any_request() {
        assert_read_only_refuses(&[DOMAIN]);
    }

    #[test]
    fn the_overview_names_the_site() {
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(status(&config(CONFIG)), "dd eu");
        assert_eq!(status(&config("[dd]\n")), "dd us1");
        assert_eq!(status(&config("")), "dd not set up");
        assert_eq!(
            status(&config("[datadog]\nsite = \"dd.contoso.example\"\n")),
            "dd config broken"
        );
    }

    #[test]
    fn a_token_goes_as_bearer_to_the_site_api_host_and_never_prints() {
        let (outcome, transport) = dd(
            &["dd", "monitor", "get", "4711"],
            vec![Answer::status(
                403,
                format!(r#"{{"errors":["Forbidden: token {TOKEN} lacks monitors_read"]}}"#),
            )],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("lacks monitors_read"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("hint: the credential lacks a scope"),
            "{}",
            outcome.stderr
        );
        let sent = &transport.sent()[0];
        assert!(
            sent.url
                .starts_with("https://api.datadoghq.eu/api/v1/monitor/4711?")
        );
        assert_eq!(
            sent.authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
    }

    #[test]
    fn a_spent_token_is_fetched_again_from_its_command_once() {
        let config = "[datadog]\ntoken_cmd = \"printf 'pup-oauth-token-1a2b3c\\\\n'\"\n";
        let (outcome, transport) = dd_with(
            config,
            &["dd", "incident", "list", "--fields", "id"],
            vec![
                Answer::status(401, r#"{"errors":["Unauthorized"]}"#),
                Answer::json(&json!({"data": {"attributes": {"incidents": []}}})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        assert!(
            sent[0].url.starts_with("https://api.datadoghq.com/"),
            "default site"
        );
        assert_eq!(
            sent[1].authorization.as_deref(),
            Some("Bearer pup-oauth-token-1a2b3c")
        );
        assert!(!outcome.stdout.contains("pup-oauth") && !outcome.stderr.contains("pup-oauth"));
    }

    /// Records the headers the fake transport does not.
    #[derive(Clone, Default)]
    struct Headers(std::sync::Arc<std::sync::Mutex<Vec<Seen>>>);

    /// A URL and the headers sent with it.
    type Seen = (String, Vec<(String, String)>);

    impl Transport for Headers {
        fn send(
            &self,
            request: &agent_cli_core::Request<'_>,
            _: Option<&agent_cli_core::Secret>,
            _: std::time::Duration,
        ) -> anyhow::Result<agent_cli_core::Response> {
            self.0
                .lock()
                .unwrap()
                .push((request.url.clone(), request.headers.clone()));
            Ok(agent_cli_core::Response {
                status: 200,
                body: r#"{"valid": true, "monitors": []}"#.to_owned(),
                ..Default::default()
            })
        }
    }

    #[test]
    fn the_key_pair_goes_in_its_headers_only_to_the_api_host_and_is_masked_everywhere() {
        let config =
            "[datadog]\nsite = \"us5.datadoghq.com\"\napi_key_env = \"K1\"\napp_key_env = \"K2\"\n";
        let headers = Headers::default();
        let setup = Setup::fake(headers.clone())
            .with_config(config)
            .with_env("K1", "api-key-value-0f0f0f")
            .with_env("K2", "app-key-value-e1e1e1");
        let outcome = run(
            &[DOMAIN],
            &["dd", "monitor", "list", "--fields", "id"],
            setup,
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let seen = headers.0.lock().unwrap().clone();
        assert!(
            seen[0]
                .0
                .starts_with("https://api.us5.datadoghq.com/api/v1/monitor/search?")
        );
        let header = |name: &str| {
            seen[0]
                .1
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(
            header("DD-API-KEY").as_deref(),
            Some("api-key-value-0f0f0f")
        );
        assert_eq!(
            header("DD-APPLICATION-KEY").as_deref(),
            Some("app-key-value-e1e1e1")
        );

        let plan = agent_cli_core::testing::assert_dry_run(
            &[DOMAIN],
            &[
                "dd",
                "downtime",
                "cancel",
                "00000000-0000-4000-8000-00000000d001",
            ],
            vec![],
        );
        assert_eq!(plan[0]["method"], "DELETE");
        let setup = Setup::fake(FakeTransport::default())
            .with_config(config)
            .with_env("K1", "api-key-value-0f0f0f")
            .with_env("K2", "app-key-value-e1e1e1");
        let outcome = run(
            &[DOMAIN],
            &[
                "dd",
                "downtime",
                "create",
                "--monitor",
                "4711",
                "--for",
                "30m",
                "--dry-run",
            ],
            setup,
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let would = &outcome.json()["would"][0];
        assert_eq!(would["headers"]["DD-API-KEY"], "***");
        assert_eq!(would["headers"]["DD-APPLICATION-KEY"], "***");
        assert!(!outcome.stdout.contains("key-value"), "{}", outcome.stdout);
    }

    #[test]
    fn the_credential_is_never_sent_to_a_host_other_than_the_site_api() {
        let transport = FakeTransport::default();
        let setup = Setup::fake(transport.clone())
            .with_config(CONFIG)
            .with_env("DD_TEST_TOKEN", TOKEN);
        let ctx = agent_cli_core::testing::ctx(setup);
        let dd = Dd::load(&ctx).unwrap();
        for url in [
            "https://api.datadoghq.com/api/v1/validate",
            "https://evil.example/api.datadoghq.eu/",
            "https://api.datadoghq.eu.evil.example/api/v1/validate",
            "http://api.datadoghq.eu/api/v1/validate",
        ] {
            let error = dd
                .send(&ctx, None, Method::Get, url.to_owned(), None)
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("refusing to send the Datadog credential"),
                "{url}: {error}"
            );
        }
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn a_literal_key_in_the_file_an_unknown_site_and_no_credential_are_setup_errors() {
        for (config, want) in [
            (
                "[datadog]\napi_key = \"abc\"\napp_key_env = \"K\"\n",
                "holds the key itself",
            ),
            (
                "[datadog]\nsite = \"datadog.contoso.example\"\n",
                "is not a Datadog site",
            ),
            ("[datadog]\napi_key_env = \"K\"\n", "the pair needs both"),
            (
                "[datadog]\nsite = \"datadoghq.eu\"\n",
                "no Datadog credential",
            ),
        ] {
            let transport = FakeTransport::default();
            let setup = Setup::fake(transport.clone()).with_config(config);
            let outcome = run(&[DOMAIN], &["dd", "monitor", "get", "4711"], setup);
            assert_eq!(outcome.code, 3, "{config}: {outcome:?}");
            assert!(
                outcome.stderr.contains(want),
                "{config}: {}",
                outcome.stderr
            );
            assert!(transport.sent().is_empty(), "{config}");
        }
    }

    #[test]
    fn doctor_names_the_site_and_the_credential_source_never_its_value() {
        let (outcome, transport) = dd(
            &["doctor", "dd"],
            vec![Answer::json(
                &json!({"monitors": [], "metadata": {"total_count": 0}}),
            )],
        );
        let rows = outcome.json();
        let checks: Vec<(&str, bool)> = rows
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["domain"] == "dd")
            .map(|row| (row["check"].as_str().unwrap(), row["ok"].as_bool().unwrap()))
            .collect();
        assert_eq!(
            &checks[..3],
            [("config", true), ("credential", true), ("connection", true)]
        );
        assert!(
            outcome.stdout.contains("token_env DD_TEST_TOKEN"),
            "{}",
            outcome.stdout
        );
        assert!(
            transport.sent()[0]
                .url
                .contains("/api/v1/monitor/search?per_page=1")
        );
    }
}
