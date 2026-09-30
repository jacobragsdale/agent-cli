use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Failure, Method, When, command, redact_value, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::client::{Airflow, At, Want};

use super::run_row;

#[derive(clap::Args)]
pub struct RunCreateArgs {
    /// The DAG: its id, or its Airflow UI URL
    dag: String,
    /// The run's conf: a JSON object, or - to read it from stdin
    #[arg(long)]
    conf: Option<String>,
    /// The run's conf from a JSON file
    #[arg(long)]
    conf_file: Option<PathBuf>,
    /// RFC 3339 or now; a DAG that templates {{ ds }} needs one (none by default)
    #[arg(long)]
    logical_date: Option<String>,
    /// A run id of your own; Airflow makes a manual__ one otherwise
    #[arg(long)]
    run_id: Option<String>,
    /// A note on the run
    #[arg(long)]
    note: Option<String>,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunCreated {
    /// DAG/RUN: what run wait takes.
    id: String,
    state: Option<String>,
    run_after: Option<String>,
    logical_date: Option<String>,
    conf: Value,
}

fn run_create(ctx: &Ctx, args: RunCreateArgs) -> Result<RunCreated> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = airflow.locate(
        ctx,
        args.at.instance.as_deref(),
        &args.dag,
        Want::Dag,
        None,
        None,
    )?;
    client.writable()?;
    let conf = match ctx.long_text(
        "conf",
        args.conf.as_deref(),
        args.conf_file.as_deref(),
        None,
    )? {
        None => json!({}),
        Some(raw) => serde_json::from_str::<Map<String, Value>>(&raw.text)
            .map(Value::Object)
            .map_err(|error| Failure::usage(format!("--conf is not a JSON object: {error}")))?,
    };
    // Since 3.0 a REST trigger without one has no data interval, so a DAG
    // that templates {{ ds }} fails; null is still the API's own default.
    let logical_date = match args.logical_date.as_deref() {
        None => Value::Null,
        Some(raw) => raw
            .parse::<When>()
            .map(|when| Value::String(when.utc()))
            .map_err(|why| Failure::usage(format!("--logical-date: {why}")))?,
    };
    let dag = client.get(&id.dag_path())?;
    if dag["is_paused"].as_bool() == Some(true) {
        ctx.note(format!(
            "[DAG {} is paused: the run stays queued until agent-cli airflow dag update {} --paused false]",
            id.dag, id.dag
        ));
    }
    let mut body = json!({"logical_date": logical_date, "conf": conf});
    if let Some(run_id) = &args.run_id {
        body["dag_run_id"] = json!(run_id);
    }
    if let Some(note) = &args.note {
        body["note"] = json!(note);
    }
    let run = client
        .change(
            Effect::Write,
            Method::Post,
            &format!("{}/dagRuns", id.dag_path()),
            body,
        )
        .map_err(|error| match status_of(&error) {
            Some(400) if format!("{error:#}").contains("import errors") => {
                refine(error, "agent-cli airflow import-error list")
            }
            // 3.3 words a duplicate as an object `detail` ("Unique
            // constraint violation"), which reads as noise.
            Some(409) => refine(
                Failure::conflict(format!(
                    "DAG {} already has a run with that run id or logical date",
                    id.dag
                ))
                .into(),
                &format!("agent-cli airflow run list --dag {}", id.dag),
            ),
            _ => error,
        })?;
    let row = run_row(&run);
    ctx.note(format!("[next: agent-cli airflow run wait {}]", row.id));
    Ok(RunCreated {
        id: row.id,
        state: row.state,
        run_after: row.run_after,
        logical_date: row.logical_date,
        conf: redact_value(run["conf"].clone()),
    })
}

/// The same failure with a better next step.
fn refine(error: anyhow::Error, hint: &str) -> anyhow::Error {
    match error.downcast::<Failure>() {
        Ok(mut failure) => {
            failure.hint = Some(hint.to_owned());
            failure.into()
        }
        Err(error) => error,
    }
}

command! {
    pub RUN_CREATE = ["airflow", "run", "create"], Write,
    "Trigger a DAG run, with a conf and optionally a logical date",
    keywords: ["trigger", "start", "kick", "off", "conf", "manual", "launch", "execute"],
    example: "airflow run create etl_nightly --conf '{\"day\":\"2026-09-28\"}'",
    run: run_create,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{airflow, dry_run, run};

    #[test]
    fn run_create_sends_a_null_logical_date_and_notes_a_paused_dag() {
        let paused = crate::testing::dag("etl_nightly", true);
        let (plans, stderr) = dry_run(
            &[
                "airflow",
                "run",
                "create",
                "etl_nightly",
                "--conf",
                "{\"day\":\"2026-09-28\"}",
                "--note",
                "rerun for finance",
            ],
            vec![Answer::json(&paused)],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            "https://airflow.contoso.example/api/v2/dags/etl_nightly/dagRuns"
        );
        assert_eq!(
            plans[0]["body"],
            json!({"logical_date": null, "conf": {"day": "2026-09-28"}, "note": "rerun for finance"})
        );
        assert!(
            stderr.contains("[DAG etl_nightly is paused: the run stays queued until agent-cli airflow dag update etl_nightly --paused false]"),
            "{stderr}"
        );

        let active = crate::testing::dag("etl_nightly", false);
        let mut created = run("queued");
        created["dag_run_id"] = json!("manual__2026-09-29T12:00:00+00:00");
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "run",
                "create",
                "etl_nightly",
                "--logical-date",
                "now",
            ],
            vec![Answer::json(&active), Answer::json(&created)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["id"],
            "etl_nightly/manual__2026-09-29T12:00:00+00:00"
        );
        let body = transport.sent()[1].body.clone().unwrap();
        assert!(
            body["logical_date"].as_str().unwrap().ends_with('Z'),
            "{body}"
        );
        assert!(
            outcome.stderr.contains(
                "[next: agent-cli airflow run wait etl_nightly/manual__2026-09-29T12:00:00+00:00]"
            ),
            "{}",
            outcome.stderr
        );

        for (status, body, code, hint) in [
            (
                409,
                r#"{"detail":{"reason":"Unique constraint violation","statement":"hidden"}}"#,
                5,
                "agent-cli airflow run list --dag etl_nightly",
            ),
            (
                400,
                r#"{"detail":"DAG with dag_id: 'etl_nightly' has import errors and cannot be triggered"}"#,
                2,
                "agent-cli airflow import-error list",
            ),
        ] {
            let (outcome, _) = airflow(
                &["airflow", "run", "create", "etl_nightly"],
                vec![Answer::json(&active), Answer::status(status, body)],
            );
            assert_eq!(outcome.code, code, "{outcome:?}");
            assert!(
                outcome.stderr.contains(&format!("hint: {hint}")),
                "{}",
                outcome.stderr
            );
        }
        let (outcome, transport) = airflow(
            &["airflow", "run", "create", "etl_nightly", "--conf", "[1]"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());

        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("conf.json");
        std::fs::write(&conf, "{\"day\": \"2026-09-28\"}\n").unwrap();
        let (plans, _) = dry_run(
            &[
                "airflow",
                "run",
                "create",
                "etl_nightly",
                "--conf-file",
                conf.to_str().unwrap(),
            ],
            vec![Answer::json(&active)],
        );
        assert_eq!(plans[0]["body"]["conf"], json!({"day": "2026-09-28"}));
    }
}
