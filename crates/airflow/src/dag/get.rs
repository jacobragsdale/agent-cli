use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Airflow, At, Want, seconds, stamp, text};
use crate::run::run_after;

use super::dag_row;

#[derive(clap::Args)]
pub struct DagGetArgs {
    /// The DAG: its id, or its Airflow UI URL
    dag: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DagDetail {
    id: String,
    paused: bool,
    schedule: Option<String>,
    /// The schedule in words.
    schedule_text: Option<String>,
    next_run: Option<String>,
    next_logical_date: Option<String>,
    /// Unpausing runs every missed interval.
    catchup: bool,
    max_active_runs: Option<i64>,
    owners: Vec<String>,
    tags: Vec<String>,
    file: Option<String>,
    bundle: Option<String>,
    /// The DAG version number.
    version: Option<i64>,
    last_parsed: Option<String>,
    import_errors: bool,
    /// True when the DAG's file is gone: Airflow starts no runs of it.
    stale: Option<bool>,
    description: Option<String>,
    /// What run create --conf may set.
    params: Vec<Param>,
    /// The last five runs, newest first.
    recent_runs: Vec<RecentRun>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Param {
    name: String,
    default: Value,
    description: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RecentRun {
    /// DAG/RUN: what run get takes.
    id: String,
    state: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    run_after: Option<String>,
    /// Seconds.
    duration: Option<i64>,
}

fn dag_get(ctx: &Ctx, args: DagGetArgs) -> Result<DagDetail> {
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = airflow.locate(
        ctx,
        args.at.instance.as_deref(),
        &args.dag,
        Want::Dag,
        None,
        None,
    )?;
    let dag = super::details(&client, &id)?;
    let runs = client.get(&format!(
        "{}/dagRuns?{}&limit=5",
        id.dag_path(),
        client.newest_first()?
    ))?;
    let params = dag["params"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, param)| {
            let described = param.is_object() && param.get("value").is_some();
            Param {
                name: name.clone(),
                default: if described {
                    param["value"].clone()
                } else {
                    param.clone()
                },
                description: text(&param["description"]),
            }
        })
        .collect();
    let row = dag_row(&dag);
    if row.stale.is_some() {
        ctx.note(format!(
            "[{} is stale: its file is gone from the dags folder, so Airflow starts no runs of it]",
            row.id
        ));
    }
    Ok(DagDetail {
        id: row.id,
        paused: row.paused,
        schedule: row.schedule,
        schedule_text: text(&dag["timetable_description"]),
        next_run: row.next_run,
        next_logical_date: stamp(&dag["next_dagrun_logical_date"])
            .or_else(|| stamp(&dag["next_dagrun"])),
        catchup: dag["catchup"].as_bool().unwrap_or_default(),
        max_active_runs: dag["max_active_runs"].as_i64(),
        owners: row.owners,
        tags: row.tags,
        file: row.file,
        bundle: text(&dag["bundle_name"]),
        version: dag["latest_dag_version"]["version_number"].as_i64(),
        last_parsed: stamp(&dag["last_parsed_time"]),
        import_errors: row.import_errors,
        stale: row.stale,
        description: text(&dag["description"]),
        params,
        recent_runs: runs["dag_runs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|run| RecentRun {
                id: format!(
                    "{}/{}",
                    run["dag_id"].as_str().unwrap_or(&id.dag),
                    run["dag_run_id"].as_str().unwrap_or_default()
                ),
                state: text(&run["state"]),
                kind: text(&run["run_type"]),
                run_after: run_after(run),
                duration: seconds(&run["start_date"], &run["end_date"]),
            })
            .collect(),
    })
}

command! {
    pub DAG_GET = ["airflow", "dag", "get"], Read,
    "Show a DAG's status: paused, next run, params, schedule and its last five runs",
    keywords: ["status", "next", "params", "schedule", "recent", "when", "catchup", "details"],
    example: "airflow dag get etl_nightly --fields paused,next_run,recent_runs",
    run: dag_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testing::{airflow, dag, paths};

    fn run(id: &str, state: &str) -> Value {
        json!({"dag_run_id": id, "dag_id": "etl_nightly", "state": state, "run_type": "scheduled",
            "run_after": "2026-09-29T00:00:00+00:00", "start_date": "2026-09-29T00:00:05.1+00:00",
            "end_date": "2026-09-29T00:43:58.2+00:00"})
    }

    #[test]
    fn dag_get_joins_the_details_params_and_recent_runs() {
        let mut details = dag("etl_nightly", false);
        details["catchup"] = json!(false);
        details["params"] = json!({
            "day": {"__class": "airflow.sdk.definitions.param.Param", "value": null,
                "description": "The day to load", "schema": {"type": ["null", "string"]}},
            "full": {"__class": "airflow.sdk.definitions.param.Param", "value": false, "schema": {}}
        });
        details["latest_dag_version"] = json!({"version_number": 7});
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "dag",
                "get",
                "https://airflow.contoso.example/dags/etl_nightly/runs?x=1",
            ],
            vec![
                Answer::json(&details),
                Answer::json(
                    &json!({"dag_runs": [run("scheduled__2026-09-29T00:00:00+00:00", "failed")], "total_entries": 1}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["params"],
            json!([
                {"name": "day", "description": "The day to load"},
                {"name": "full", "default": false}
            ])
        );
        assert_eq!(got["version"], 7);
        assert_eq!(got["last_parsed"], "2026-09-29T11:58:00Z");
        assert_eq!(
            got["recent_runs"],
            json!([{"id": "etl_nightly/scheduled__2026-09-29T00:00:00+00:00", "state": "failed",
                "type": "scheduled", "run_after": "2026-09-29T00:00:00Z", "duration": 2633}])
        );
        assert_eq!(
            paths(&transport),
            [
                "dags/etl_nightly/details",
                "dags/etl_nightly/dagRuns?order_by=-run_after&limit=5"
            ]
        );
    }

    #[test]
    fn a_missing_dag_is_exit_4_hinting_the_list() {
        let (outcome, _) = airflow(
            &["airflow", "dag", "get", "nope"],
            vec![Answer::status(
                404,
                r#"{"detail":"The DAG with dag_id: `nope` was not found"}"#,
            )],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("was not found\nhint: agent-cli airflow dag list"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_url_on_an_unconfigured_host_is_exit_2_before_any_request() {
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "dag",
                "get",
                "https://other.contoso.example/dags/etl_nightly",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("configured: prod (https://airflow.contoso.example)"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn a_dag_whose_file_is_gone_says_it_is_stale() {
        let mut details = dag("e2e_wide", false);
        details["is_stale"] = json!(true);
        let (outcome, _) = airflow(
            &["airflow", "dag", "get", "e2e_wide", "--fields", "id,stale"],
            vec![
                Answer::json(&details),
                Answer::json(&json!({"dag_runs": [], "total_entries": 0})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!({"id": "e2e_wide", "stale": true}));
        assert!(
            outcome
                .stderr
                .contains("[e2e_wide is stale: its file is gone"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn dag_get_on_airflow_2_orders_runs_by_logical_date_and_finds_a_stale_dag() {
        let mut details = crate::testing::dag_v1("etl_nightly", false);
        details["catchup"] = json!(true);
        details["params"] = json!({"day": {"__class": "airflow.models.param.Param",
            "value": null, "description": "The day to load", "schema": {}}});
        let (outcome, transport) = crate::testing::airflow_v1(
            &["airflow", "dag", "get", "etl_nightly"],
            vec![
                Answer::json(&details),
                Answer::json(
                    &json!({"dag_runs": [crate::testing::run_v1("failed")], "total_entries": 1}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["next_logical_date"], "2026-09-29T00:00:00Z");
        assert_eq!(got["params"][0]["description"], "The day to load");
        assert_eq!(
            got["recent_runs"][0]["run_after"], "2026-09-29T00:00:00Z",
            "a scheduled run is due when its interval ends"
        );
        assert_eq!(
            paths(&transport),
            [
                "dags/etl_nightly/details",
                "dags/etl_nightly/dagRuns?order_by=-execution_date&limit=5"
            ]
        );

        // Airflow 2 reads details from the parsed DAG: a stale one is a 404.
        let mut stale = crate::testing::dag_v1("e2e_wide", false);
        stale["is_active"] = json!(false);
        let (outcome, transport) = crate::testing::airflow_v1(
            &["airflow", "dag", "get", "e2e_wide", "--fields", "id,stale"],
            vec![
                Answer::status(
                    404,
                    r#"{"detail": "The DAG with dag_id: e2e_wide was not found", "status": 404}"#,
                ),
                Answer::json(&stale),
                Answer::json(&json!({"dag_runs": [], "total_entries": 0})),
            ],
        );
        assert_eq!(
            outcome.json(),
            json!({"id": "e2e_wide", "stale": true}),
            "{outcome:?}"
        );
        assert_eq!(paths(&transport)[1], "dags/e2e_wide");
    }
}
