//! `airflow run diff`: two DAG runs (the good one against the failed one),
//! or one against the last successful run before it: state, conf, DAG
//! version, and each task's state, tries, duration, pool, queue, executor
//! config (a pod override's image) and the XCom keys it pushed. Values stay
//! with `airflow xcom get`; a conf key named like a secret is compared, not
//! printed.

use std::collections::BTreeMap;

use agent_cli_core::{
    Ctx, Diff, DiffArgs, Failure, Rules, Secret, Side, command, diff, scoped, sensitive_key,
};
use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::client::{Airflow, At, Client, Ref, Want, text};

use super::run_after;

#[derive(clap::Args)]
pub struct RunDiffArgs {
    /// One side: [INSTANCE:]DAG/RUN (DAG/latest for the newest)
    left: String,
    /// The other side, as left; left out, LEFT against the last successful run before it
    right: Option<String>,
    #[command(flatten)]
    at: At,
    #[command(flatten)]
    diff: DiffArgs,
}

/// Two runs of one DAG are no two environments: their differing words are
/// dates.
const RULES: Rules = Rules {
    keys: &[("tasks", "task_id")],
    noise: &[],
    infer: false,
    ..Rules::PLAIN
};

/// The most task instances and XComs read of one run.
const MOST: usize = 1000;

fn run_diff(ctx: &Ctx, args: RunDiffArgs) -> Result<Diff> {
    let airflow = Airflow::load(ctx.config())?;
    let locate = |raw: &str| {
        let (scope, rest) = scoped(raw);
        let wanted = scope.or(args.at.instance.as_deref());
        airflow.locate(ctx, wanted, rest, Want::Run, None, None)
    };
    let (right_client, right_id) = locate(args.right.as_deref().unwrap_or(&args.left))?;
    let right_run = right_client.get(&right_id.run_path())?;
    let other;
    let (left_client, left_id, left_run): (&Client, Ref, Value) = match &args.right {
        Some(_) => {
            let (client, id) = locate(&args.left)?;
            let run = client.get(&id.run_path())?;
            other = client;
            (&other, id, run)
        }
        None => {
            let (id, run) = good_before(&right_client, &right_id, &right_run)?;
            (&right_client, id, run)
        }
    };
    let (left, left_secrets) = row(left_client, &left_id, &left_run)?;
    let (right, right_secrets) = row(&right_client, &right_id, &right_run)?;
    ratios(ctx, &left, &right);
    // A task that failed on the right is the one to read next.
    let failed = right["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|task| task["state"] == "failed");
    if let Some(task) = failed {
        ctx.note(format!(
            "[next: agent-cli airflow task logs {}/{}/{}/{}{}]",
            right_id.dag,
            right_id.run,
            task["task_id"].as_str().unwrap_or_default(),
            task["try_number"],
            right_client.instance.flag()
        ));
    }
    let side = |client: &Client, id: &Ref, row, secrets| Side {
        id: format!("{}:{}/{}", client.instance.name, id.dag, id.run),
        row,
        secrets,
    };
    diff(
        ctx,
        &args.diff,
        &RULES,
        side(left_client, &left_id, left, left_secrets),
        side(&right_client, &right_id, right, right_secrets),
    )
}

/// The last successful run of the DAG due before `run`.
fn good_before(client: &Client, id: &Ref, run: &Value) -> Result<(Ref, Value)> {
    let due = run_after(run);
    let answer = client.get(&format!(
        "{}/dagRuns?{}&state=success&limit=25",
        id.dag_path(),
        client.newest_first()?
    ))?;
    let found = answer["dag_runs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|held| held["dag_run_id"].as_str() != Some(id.run.as_str()) && run_after(held) < due);
    let Some(found) = found else {
        return Err(Failure::not_found(format!(
            "{} has no successful run before {}",
            id.dag, id.run
        ))
        .hint(format!(
            "agent-cli airflow run list --dag {} --state success",
            id.dag
        ))
        .into());
    };
    let good = Ref {
        run: found["dag_run_id"].as_str().unwrap_or_default().to_owned(),
        ..id.clone()
    };
    Ok((good, found.clone()))
}

/// A run as its diff compares it, and its secret conf values by path.
fn row(
    client: &Client,
    id: &Ref,
    run: &Value,
) -> Result<(Value, BTreeMap<String, Option<Secret>>)> {
    let mut conf = Map::new();
    let mut secrets = BTreeMap::new();
    for (key, value) in run["conf"].as_object().into_iter().flatten() {
        if sensitive_key(key) {
            secrets.insert(format!("conf.{key}"), Some(Secret::new(value.to_string())));
        } else {
            conf.insert(key.clone(), value.clone());
        }
    }
    let (tasks, _) = client.list(
        &format!("{}/taskInstances", id.run_path()),
        "",
        "task_instances",
        MOST,
    )?;
    // VERIFY(work): `~` for every task, as dagRuns and taskInstances take it.
    let (xcoms, _) = client.list(
        &format!("{}/taskInstances/~/xcomEntries", id.run_path()),
        "",
        "xcom_entries",
        MOST,
    )?;
    let tasks: Vec<Value> = tasks
        .iter()
        .map(|task| {
            let name = task["task_id"].as_str().unwrap_or_default();
            let mut keys: Vec<&str> = xcoms
                .iter()
                .filter(|xcom| xcom["task_id"] == name)
                .filter_map(|xcom| xcom["key"].as_str())
                .collect();
            keys.sort_unstable();
            keys.dedup();
            json!({
                "task_id": name,
                "state": task["state"],
                "try_number": task["try_number"],
                "duration": task["duration"].as_f64().map(|seconds| seconds.round() as i64),
                "operator": task["operator"],
                "pool": task["pool"],
                "queue": task["queue"],
                "executor_config": text(&task["executor_config"]).filter(|config| config != "{}"),
                "xcom": keys,
            })
        })
        .collect();
    let row = json!({
        "state": run["state"],
        "type": run["run_type"],
        "triggered_by": run["triggered_by"],
        "dag_version": run["dag_versions"].as_array().and_then(|held| held.last())
            .and_then(|version| version["version_number"].as_i64()),
        "duration": crate::client::seconds(&run["start_date"], &run["end_date"]),
        "conf": conf,
        "tasks": tasks,
    });
    Ok((row, secrets))
}

/// A task that took half as long again, or under two thirds: the duration
/// alone says by how much only after dividing.
fn ratios(ctx: &Ctx, left: &Value, right: &Value) {
    let mut said = Vec::new();
    for task in right["tasks"].as_array().into_iter().flatten() {
        let before = left["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|held| held["task_id"] == task["task_id"])
            .and_then(|held| held["duration"].as_i64());
        if let (Some(before), Some(after)) = (before.filter(|b| *b > 0), task["duration"].as_i64())
        {
            let ratio = after as f64 / before as f64;
            if !(0.67..=1.5).contains(&ratio) {
                said.push(format!(
                    "{} {ratio:.1}x",
                    task["task_id"].as_str().unwrap_or_default()
                ));
            }
        }
    }
    if !said.is_empty() {
        ctx.note(format!("[duration right/left: {}]", said.join(", ")));
    }
}

command! {
    pub RUN_DIFF = ["airflow", "run", "diff"], Read,
    "Compare two DAG runs (a good one, a failed one): conf, states, tries, durations",
    keywords: ["good", "bad", "failed", "succeeded", "changed", "previous", "successful", "duration"],
    example: "airflow run diff etl_nightly/latest",
    run: run_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testing::{RUN, airflow, paths, run, ti, tis};

    fn good() -> Value {
        let mut held = run("success");
        held["dag_run_id"] = json!("scheduled__2026-09-28T00:00:00+00:00");
        held["run_after"] = json!("2026-09-28T00:00:00+00:00");
        held["start_date"] = json!("2026-09-28T00:00:05+00:00");
        held["end_date"] = json!("2026-09-28T00:20:05+00:00");
        held
    }

    #[test]
    fn one_run_compares_with_the_last_good_one_and_secrets_in_conf_stay_hidden() {
        let mut failed = ti("load_orders", "failed", 2);
        failed["duration"] = json!(400.2);
        let mut fine = ti("load_orders", "success", 1);
        fine["duration"] = json!(100.0);
        let xcoms = |keys: &[&str]| {
            Answer::json(
                &json!({"xcom_entries": keys.iter().map(|key| json!({"key": key,
                "task_id": "load_orders"})).collect::<Vec<_>>(), "total_entries": keys.len()}),
            )
        };
        let (outcome, transport) = airflow(
            &["airflow", "run", "diff", &format!("etl_nightly/{RUN}")],
            vec![
                Answer::json(&run("failed")),
                Answer::json(&json!({"dag_runs": [run("success"), good()], "total_entries": 2})),
                tis(vec![fine]),
                xcoms(&["return_value"]),
                tis(vec![failed]),
                xcoms(&[]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["left"],
            "prod:etl_nightly/scheduled__2026-09-28T00:00:00+00:00"
        );
        assert_eq!(
            got["changes"],
            json!([
                {"path": "state", "left": "success", "right": "failed"},
                {"path": "duration", "left": 1200, "right": 2633},
                {"path": "tasks.load_orders.state", "left": "success", "right": "failed"},
                {"path": "tasks.load_orders.try_number", "left": 1, "right": 2},
                {"path": "tasks.load_orders.duration", "left": 100, "right": 400},
                {"path": "tasks.load_orders.xcom", "left": ["return_value"]},
            ])
        );
        assert!(
            outcome
                .stderr
                .contains("[duration right/left: load_orders 4.0x]"),
            "{}",
            outcome.stderr
        );
        assert!(!outcome.stdout.contains("s3cr3t"), "{}", outcome.stdout);
        assert_eq!(
            paths(&transport)[1],
            "dags/etl_nightly/dagRuns?order_by=-run_after&state=success&limit=25"
        );
    }
}
