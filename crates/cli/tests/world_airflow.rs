//! What the contoso world answers of airflow: the airflow hops of the chains
//! flows (F6, why a task failed in its code and its input; F7, why tasks
//! queue and where a DAG's data goes). The failed-DAG trace to k8s is in
//! `world_cross.rs`. Compiled only with the `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::{follow, ok};

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
        "--instance",
        "prod",
    ]);
    assert_eq!(log["at"], "etl_nightly:42");

    let source = ok(&[
        "airflow",
        "source",
        "get",
        log["at"].as_str().unwrap(),
        "--fields",
        "id,lines,text,repo_file",
        "--instance",
        "prod",
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
        "--instance",
        "prod",
    ]);
    let id = format!("{RUN}/extract_orders@return_value");
    assert_eq!(keys[1], json!({"id": id}));
    let input = ok(&[
        "airflow",
        "xcom",
        "get",
        &id,
        "--fields",
        "value",
        "--instance",
        "prod",
    ]);
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
    let run = world::agent_cli(&[
        "airflow",
        "run",
        "get",
        "etl_nightly/latest",
        "--instance",
        "prod",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let failed = format!("{RUN}/load_orders/2");
    assert!(
        run.stderr.contains(&format!(
            "[next: agent-cli airflow task logs {failed} --instance prod]"
        )),
        "{}",
        run.stderr
    );
    let log = ok(&[
        "airflow",
        "task",
        "logs",
        &failed,
        "--fields",
        "at",
        "--instance",
        "prod",
    ]);
    assert_eq!(log["at"], "etl_nightly:42");
}

#[test]
fn an_import_error_names_its_line_in_the_dags_repo() {
    let listed = ok(&[
        "airflow",
        "import-error",
        "list",
        "--fields",
        "id,file",
        "--instance",
        "prod",
    ]);
    assert_eq!(listed[0], json!({"id": 12, "file": "customer_sync.py"}));
    let got = world::agent_cli(&["airflow", "import-error", "get", "12", "--instance", "prod"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert_eq!(got.json()["line"], 5);
    assert_eq!(
        got.json()["repo_file"],
        "airflow-dags:dags/customer_sync.py:5"
    );
    assert!(
        got.stderr
            .contains("[next: agent-cli ado file get airflow-dags:dags/customer_sync.py:5]"),
        "{}",
        got.stderr
    );
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
        "--instance",
        "prod",
    ]);
    assert_eq!(task, json!({"pool": "default_pool"}));
    let pools = ok(&[
        "airflow",
        "pool",
        "list",
        "--fields",
        "id,open,queued",
        "--instance",
        "prod",
    ]);
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
        "--instance",
        "prod",
    ]);
    assert_eq!(
        connections[2],
        json!({"id": "reporting_dw", "type": "mssql", "host": "sql.contoso.example",
            "sql_conn": "reporting"})
    );
    let variables = world::agent_cli(&["airflow", "variable", "list", "--instance", "prod"]);
    assert_eq!(variables.code, 0, "{}", variables.stderr);
    assert!(
        variables.stdout.contains("orders_api_token") && !variables.stdout.contains("5000"),
        "keys, never values: {}",
        variables.stdout
    );
}

#[test]
fn an_import_error_is_found_by_its_dag_file() {
    let error = ok(&[
        "airflow",
        "import-error",
        "get",
        "customer_sync.py",
        "--fields",
        "id,line",
        "--instance",
        "prod",
    ]);
    assert_eq!(error["id"], 12);
    assert_eq!(error["line"], 5);
}

/// qa's `etl_nightly.py` logs and skips an order without a customer at line
/// 42 where prod's raises, qa has no `orders_batch_size`, and the failed run
/// compared with the good one before it leads to its failed task's log.
#[test]
fn qa_and_prod_differ_at_line_42_and_the_failed_run_differs_from_the_good_one() {
    let dag = ok(&[
        "airflow",
        "dag",
        "diff",
        "qa:etl_nightly",
        "prod:etl_nightly",
    ]);
    let lines = dag["changes"][0]["lines"].as_array().unwrap();
    assert_eq!(dag["changes"][0]["path"], "source");
    assert_eq!(lines[0], "@@ -39,7 +39,7 @@");
    assert!(
        lines.contains(&json!(
            "+            raise ValueError(f\"order {order_id} has no customer_id\")"
        )),
        "{dag}"
    );
    let variables = ok(&["airflow", "variable", "diff", "qa", "prod"]);
    assert_eq!(
        variables["changes"][0]["path"], "orders_batch_size",
        "{variables}"
    );
    let walked = follow(&["airflow", "run", "diff", "prod:etl_nightly/latest"]);
    let runs = walked[0].json();
    assert_eq!(
        runs["left"],
        "prod:etl_nightly/scheduled__2026-09-28T00:00:00+00:00"
    );
    assert!(
        runs["changes"].as_array().unwrap().contains(
            &json!({"path": "tasks.load_orders.state", "left": "success", "right": "failed"})
        ),
        "{runs}"
    );
    assert_eq!(walked.len(), 2, "the failed task's log is the next step");
    assert!(
        walked[1].stdout.contains("order 88123 has no customer_id"),
        "{}",
        walked[1].stdout
    );
}
