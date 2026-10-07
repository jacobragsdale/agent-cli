//! What the crate's tests share: one instance with an API token, runs of the
//! domain over it, and a job's status as the 9.22 spec shapes it.

use agent_cli_core::Setup;
use agent_cli_core::testing::{Answer, FakeTransport, Outcome};
use serde_json::{Value, json};

pub(crate) const API: &str = "https://ctm.contoso.example:8443/automation-api";
/// One instance with an API token from the environment, on servers two hours
/// ahead of UTC.
pub(crate) const CONFIG: &str = "[[controlm.instance]]\nname = \"prod\"\n\
    base_url = \"https://ctm.contoso.example:8443/automation-api\"\n\
    token_env = \"CONTROLM_TOKEN\"\nutc_offset = \"+02:00\"\n";
/// The same instance signing in with a password.
pub(crate) const PASSWORD_CONFIG: &str = "[[controlm.instance]]\nname = \"prod\"\n\
    base_url = \"https://ctm.contoso.example:8443/automation-api\"\n\
    username = \"agent\"\npassword_env = \"CONTROLM_PASSWORD\"\n";
pub(crate) const TOKEN: &str = "fixture-api-token";

pub(crate) fn controlm(argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
    controlm_with(CONFIG, argv, answers)
}

pub(crate) fn controlm_with(
    config: &str,
    argv: &[&str],
    answers: Vec<Answer>,
) -> (Outcome, FakeTransport) {
    let transport = FakeTransport::answering(answers);
    let setup = Setup::fake(transport.clone())
        .with_config(config)
        .with_env("CONTROLM_TOKEN", TOKEN)
        .with_env("CONTROLM_PASSWORD", "fixture-password");
    (
        agent_cli_core::testing::run(&[crate::DOMAIN], argv, setup),
        transport,
    )
}

/// The paths sent, after `{API}/`, in order.
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

/// `--dry-run` over [`CONFIG`]: the reads run, the first change is planned
/// and never sent.
pub(crate) fn dry_run(argv: &[&str], answers: Vec<Answer>) -> (Vec<Value>, String) {
    let mut argv = argv.to_vec();
    argv.push("--dry-run");
    let (outcome, transport) = controlm(&argv, answers);
    assert!(
        transport.sent().iter().all(|sent| sent.method.is_read()),
        "a change was sent under --dry-run"
    );
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let printed = outcome.json();
    assert_eq!(
        printed["dry_run"], true,
        "never reached ctx.write: {outcome:?}"
    );
    (
        printed["would"].as_array().cloned().unwrap_or_default(),
        outcome.stderr,
    )
}

/// A job's status (`JobRunStatus`) as the 9.22 spec names its fields, in the
/// NightlyLoads folder on ctm-prod.
// VERIFY(work): replace with a real answer from `run/jobs/status`, names
// scrubbed to contoso-style ones.
pub(crate) fn job(id: &str, name: &str, status: &str) -> Value {
    json!({
        "jobId": id, "folderId": "ctm-prod:00a10", "numberOfRuns": 2,
        "name": name, "folder": "NightlyLoads", "type": "Command", "status": status,
        "held": false, "deleted": false, "cyclic": false,
        "startTime": "20260929021500", "endTime": "20260929024355",
        "estimatedStartTime": [], "estimatedEndTime": [],
        "orderDate": "260929", "ctm": "ctm-prod", "description": "Loads the day's orders",
        "host": "etl-01", "application": "Orders", "subApplication": "Loads",
        "outputURI": format!("{API}/run/job/{id}/output"),
        "logURI": format!("{API}/run/job/{id}/log"),
    })
}
