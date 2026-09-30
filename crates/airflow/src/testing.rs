//! What the crate's tests share: one instance with a bearer token, runs of
//! the domain over it, and the DAG, run and task instance records Airflow
//! answers with.

use agent_cli_core::Setup;
use agent_cli_core::testing::{Answer, FakeTransport, Outcome};
use serde_json::{Value, json};

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
    (
        agent_cli_core::testing::run(&[crate::DOMAIN], argv, setup),
        transport,
    )
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

pub(crate) fn dag(id: &str, paused: bool) -> Value {
    json!({
        "dag_id": id, "dag_display_name": id, "is_paused": paused, "is_stale": false,
        "last_parsed_time": "2026-09-29T11:58:00.123456+00:00", "bundle_name": "dags-folder",
        "relative_fileloc": format!("{id}.py"), "fileloc": format!("/opt/airflow/dags/{id}.py"),
        "description": null, "timetable_summary": "0 0 * * *",
        "timetable_description": "At 00:00", "tags": [{"name": "etl", "dag_id": id}],
        "max_active_runs": 1, "has_import_errors": false,
        "next_dagrun_logical_date": "2026-09-30T00:00:00+00:00",
        "next_dagrun_run_after": "2026-09-30T00:00:00+00:00", "owners": ["data-eng"]
    })
}

pub(crate) const RUN: &str = "scheduled__2026-09-29T00:00:00+00:00";
pub(crate) const RUN_PATH: &str =
    "dags/etl_nightly/dagRuns/scheduled__2026-09-29T00%3A00%3A00%2B00%3A00";

pub(crate) fn run(state: &str) -> Value {
    json!({"dag_run_id": RUN, "dag_id": "etl_nightly", "state": state, "run_type": "scheduled",
        "logical_date": "2026-09-29T00:00:00+00:00", "run_after": "2026-09-29T00:00:00+00:00",
        "start_date": "2026-09-29T00:00:05.12+00:00", "end_date": "2026-09-29T00:43:58.9+00:00",
        "triggered_by": "timetable", "conf": {"day": "2026-09-28", "api_token": "s3cr3t-conf-value"},
        "note": null, "dag_versions": [{"version_number": 6}, {"version_number": 7}]})
}

pub(crate) fn ti(task: &str, state: &str, attempt: i64) -> Value {
    json!({"id": "0199", "task_id": task, "dag_id": "etl_nightly", "dag_run_id": RUN,
        "map_index": -1, "state": state, "try_number": attempt, "max_tries": 1,
        "start_date": "2026-09-29T00:41:07+00:00", "end_date": "2026-09-29T00:43:55+00:00",
        "duration": 168.4, "operator": "PythonOperator", "executor": null, "queue": "default",
        "pool": "default_pool", "hostname": "etl-nightly-load-orders-q8x1k2vz", "note": null,
        "rendered_fields": {"op_kwargs": {"day": "2026-09-28"}}})
}

pub(crate) fn tasks(items: Vec<Value>) -> Answer {
    Answer::json(&json!({"total_entries": items.len(), "task_instances": items}))
}

pub(crate) fn tis(items: Vec<Value>) -> Answer {
    Answer::json(&json!({"total_entries": items.len(), "task_instances": items}))
}
