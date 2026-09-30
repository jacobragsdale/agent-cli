//! What the crate's tests share: a config with a token, runs of the domain
//! over it, and the incidents and downtimes Datadog answers with.

use agent_cli_core::Setup;
use agent_cli_core::testing::{Answer, FakeTransport, Outcome, run};
use serde_json::{Value, json};

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

pub(crate) fn incident(public_id: i64, title: &str, state: &str) -> serde_json::Value {
    json!({"data": {
        "id": format!("00000000-0000-4000-8000-000000000{public_id}"),
        "type": "incidents",
        "attributes": {
            "public_id": public_id, "title": title, "state": state, "severity": "SEV-2",
            "customer_impacted": true,
            "created": "2026-09-28T21:40:12.000000+00:00", "detected": "2026-09-28T21:37:00+00:00",
            "resolved": null,
            "fields": {
                "severity": {"type": "dropdown", "value": "SEV-2"},
                "summary": {"type": "textbox", "value": "api 5xx during the v1.4.2 rollout"},
                "teams": {"type": "autocomplete", "value": null}
            }
        }
    }})
}

pub(crate) fn downtime(id: &str, monitor: i64, end: &str) -> Value {
    json!({
        "id": id, "type": "downtime",
        "attributes": {
            "status": "active", "scope": "*", "message": "deploy v1.4.2", "display_timezone": "UTC",
            "canceled": null, "created": "2026-09-29T11:40:00.300497+00:00",
            "monitor_identifier": {"monitor_id": monitor},
            "schedule": {"start": "2026-09-29T11:40:00.286133+00:00", "end": end}
        },
        "relationships": {"monitor": {"data": {"type": "monitors", "id": monitor.to_string()}}}
    })
}
