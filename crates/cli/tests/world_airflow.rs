//! What the contoso world answers of airflow: the airflow hops of the chains
//! flows (F6, why a task failed in its code and its input; F7, why tasks
//! queue and where a DAG's data goes). The failed-DAG trace to k8s is in
//! `world_cross.rs`. Compiled only with the `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::ok;

const RUN: &str = "etl_nightly/scheduled__2026-09-29T00:00:00+00:00";

#[test]
fn a_failed_task_leads_to_its_line_in_the_dag_and_the_input_it_failed_on() {
    let log = ok(&[
        "airflow",
        "task",
        "logs",
        "etl_nightly/latest/load_orders/2",
        "--fields",
        "error,at",
    ]);
    assert_eq!(log["at"], "etl_nightly:42");

    let source = ok(&[
        "airflow",
        "source",
        "get",
        log["at"].as_str().unwrap(),
        "--fields",
        "id,lines,text,repo_file",
    ]);
    assert_eq!(source["id"], "etl_nightly:42");
    assert_eq!(
        source["repo_file"], "airflow-dags:dags/etl_nightly.py:42",
        "the id ado file get takes, from dags_repo"
    );
    assert!(
        source["text"].as_str().unwrap().contains(
            "\n42              raise ValueError(f\"order {order_id} has no customer_id\")\n"
        ),
        "{source}"
    );

    let keys = ok(&[
        "airflow",
        "xcom",
        "list",
        "etl_nightly/latest/extract_orders",
        "--fields",
        "id",
    ]);
    let id = format!("{RUN}/extract_orders@return_value");
    assert_eq!(keys[1], json!({"id": id}));
    let input = ok(&["airflow", "xcom", "get", &id, "--fields", "value"]);
    let missing: Vec<&serde_json::Value> = input["value"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|order| order.get("customer_id").is_none())
        .collect();
    assert_eq!(missing, [&json!({"order_id": 88123, "total": 18.5})]);
}

#[test]
fn a_failed_run_names_its_failed_tasks_log_as_the_next_step() {
    let run = world::agent_cli(&["airflow", "run", "get", "etl_nightly/latest"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let failed = format!("{RUN}/load_orders/2");
    assert!(
        run.stderr
            .contains(&format!("[next: agent-cli airflow task logs {failed}]")),
        "{}",
        run.stderr
    );
    let log = ok(&["airflow", "task", "logs", &failed, "--fields", "at"]);
    assert_eq!(log["at"], "etl_nightly:42");
}

#[test]
fn a_tasks_pool_and_a_dags_connections_lead_to_the_sql_connection() {
    let task = ok(&[
        "airflow",
        "task",
        "get",
        "etl_nightly/latest/load_orders",
        "--fields",
        "pool,blocked_by",
    ]);
    assert_eq!(task, json!({"pool": "default_pool"}));
    let pools = ok(&["airflow", "pool", "list", "--fields", "id,open,queued"]);
    assert_eq!(
        pools[0],
        json!({"id": "default_pool", "open": 128, "queued": 0})
    );
    let connections = ok(&[
        "airflow",
        "connection",
        "list",
        "--fields",
        "id,type,host,sql_conn",
    ]);
    assert_eq!(
        connections[2],
        json!({"id": "reporting_dw", "type": "mssql", "host": "sql.contoso.example",
            "sql_conn": "reporting"})
    );
    let variables = world::agent_cli(&["airflow", "variable", "list"]);
    assert_eq!(variables.code, 0, "{}", variables.stderr);
    assert!(
        variables.stdout.contains("orders_api_token") && !variables.stdout.contains("5000"),
        "keys, never values: {}",
        variables.stdout
    );
}
