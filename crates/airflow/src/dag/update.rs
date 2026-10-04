use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Airflow, At, Want, stamp};

#[derive(clap::Args)]
pub struct DagUpdateArgs {
    /// The DAG: its id, or its Airflow UI URL
    dag: String,
    /// true pauses the DAG, false unpauses it
    #[arg(long, required = true, action = clap::ArgAction::Set)]
    paused: bool,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DagUpdated {
    id: String,
    paused: bool,
    next_run: Option<String>,
}

fn dag_update(ctx: &Ctx, args: DagUpdateArgs) -> Result<DagUpdated> {
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
    let dag = super::details(&client, &id)?;
    if !args.paused && dag["catchup"].as_bool() == Some(true) {
        ctx.note(format!(
            "[catchup is on: unpausing {} schedules every interval missed since {}]",
            id.dag,
            stamp(&dag["next_dagrun_logical_date"]).unwrap_or_else(|| "it was paused".to_owned())
        ));
    }
    let updated = client.change(
        Effect::Write,
        Method::Patch,
        &format!("{}?update_mask=is_paused", id.dag_path()),
        json!({"is_paused": args.paused}),
    )?;
    Ok(DagUpdated {
        id: id.dag,
        paused: updated["is_paused"].as_bool().unwrap_or(args.paused),
        next_run: super::next_run(&updated),
    })
}

command! {
    pub DAG_UPDATE = ["airflow", "dag", "update"], Write,
    "Pause or unpause a DAG",
    keywords: ["pause", "unpause", "enable", "disable", "resume", "stop", "schedule"],
    example: "airflow dag update etl_nightly --paused true",
    run: dag_update,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CONFIG, airflow, airflow_with, dag, dry_run};

    #[test]
    fn unpausing_patches_is_paused_and_notes_catchup() {
        let mut details = dag("etl_nightly", true);
        details["catchup"] = json!(true);
        let (plans, stderr) = dry_run(
            &[
                "airflow",
                "dag",
                "update",
                "etl_nightly",
                "--paused",
                "false",
            ],
            vec![Answer::json(&details)],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            "https://airflow.contoso.example/api/v2/dags/etl_nightly?update_mask=is_paused"
        );
        assert_eq!(plans[0]["body"], json!({"is_paused": false}));
        assert!(stderr.contains("[catchup is on: unpausing etl_nightly schedules every interval missed since 2026-09-30T00:00:00Z]"), "{stderr}");

        let (outcome, transport) = airflow(
            &[
                "airflow",
                "dag",
                "update",
                "etl_nightly",
                "--paused",
                "true",
            ],
            vec![
                Answer::json(&details),
                Answer::json(&dag("etl_nightly", true)),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "etl_nightly", "paused": true, "next_run": "2026-09-30T00:00:00Z"})
        );
        assert_eq!(transport.sent()[1].body, Some(json!({"is_paused": true})));
    }

    #[test]
    fn a_read_only_instance_refuses_a_change_even_as_a_dry_run() {
        let config = format!("{CONFIG}read_only = true\n");
        for extra in [None, Some("--dry-run")] {
            let mut argv = vec![
                "airflow",
                "dag",
                "update",
                "etl_nightly",
                "--paused",
                "true",
            ];
            argv.extend(extra);
            let (outcome, transport) = airflow_with(&config, &argv, vec![]);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(
                outcome.stderr.contains("instance \"prod\" is read_only"),
                "{}",
                outcome.stderr
            );
            assert!(transport.sent().is_empty());
        }
    }

    #[test]
    fn dag_update_on_airflow_2_patches_the_same_path() {
        let (plans, _) = crate::testing::dry_run_with(
            crate::testing::CONFIG_V1,
            &[
                "airflow",
                "dag",
                "update",
                "etl_nightly",
                "--paused",
                "true",
            ],
            vec![Answer::json(&crate::testing::dag_v1("etl_nightly", false))],
        );
        assert_eq!(
            plans[0]["url"],
            "https://airflow.contoso.example/api/v1/dags/etl_nightly?update_mask=is_paused"
        );
        let (outcome, _) = crate::testing::airflow_v1(
            &[
                "airflow",
                "dag",
                "update",
                "etl_nightly",
                "--paused",
                "false",
            ],
            vec![
                Answer::json(&crate::testing::dag_v1("etl_nightly", true)),
                Answer::json(&crate::testing::dag_v1("etl_nightly", false)),
            ],
        );
        assert_eq!(
            outcome.json(),
            json!({"id": "etl_nightly", "paused": false, "next_run": "2026-09-30T00:00:00Z"})
        );
    }
}
