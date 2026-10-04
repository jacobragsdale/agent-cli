//! The airflow domain: DAGs and their source, runs, task instances with
//! their logs and XComs, import errors, pools, variables and connections,
//! live from Apache Airflow 3's REST API (`/api/v2`) or 2.9's (`/api/v1`),
//! whichever the server speaks.
//!
//! Every id is the ref: a DAG is `etl_nightly`, a line of its file
//! `etl_nightly:42`, a run `DAG/RUN`, a task instance
//! `DAG/RUN/TASK[:MAP][/TRY]`, an XCom `DAG/RUN/TASK[:MAP]@KEY`, and an
//! instance with a `k8s_scope` prints the pod a task ran in as the id
//! `k8s pod logs` takes.

mod client;
mod connection;
mod dag;
mod dag_run;
mod doctor;
mod import_error;
mod instance;
mod pool;
mod refused;
mod run;
mod source;
mod task;
#[cfg(test)]
mod testing;
mod variable;
mod xcom;

use agent_cli_core::Domain;

pub const DOMAIN: Domain = Domain {
    name: "airflow",
    summary: "Apache Airflow",
    commands: &[
        instance::list::INSTANCE_LIST,
        pool::list::POOL_LIST,
        variable::list::VARIABLE_LIST,
        connection::list::CONNECTION_LIST,
        dag::list::DAG_LIST,
        dag::get::DAG_GET,
        dag::update::DAG_UPDATE,
        source::get::SOURCE_GET,
        run::list::RUN_LIST,
        run::get::RUN_GET,
        run::create::RUN_CREATE,
        run::wait::RUN_WAIT,
        run::retry::RUN_RETRY,
        task::list::TASK_LIST,
        task::get::TASK_GET,
        task::logs::TASK_LOGS,
        task::retry::TASK_RETRY,
        xcom::list::XCOM_LIST,
        xcom::get::XCOM_GET,
        import_error::list::IMPORT_ERROR_LIST,
        import_error::get::IMPORT_ERROR_GET,
    ],
    // Airflow's own phrases only: search applies them to every domain.
    synonyms: &[
        ("dag run", &["run"]),
        ("dag runs", &["run"]),
        ("dagrun", &["run"]),
        ("dagruns", &["run"]),
        ("task instance", &["task"]),
        ("task instances", &["task"]),
        ("data pipeline", &["dag"]),
        ("broken dag", &["import", "error"]),
        ("dag code", &["source"]),
        ("dag file", &["source"]),
        ("return value", &["xcom"]),
        ("task output", &["xcom"]),
        ("slots", &["pool"]),
    ],
    status: doctor::status,
    doctor: doctor::doctor,
};

#[cfg(test)]
mod tests {
    use agent_cli_core::{check_layout, check_registry};

    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(check_layout(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(DOMAIN.commands.len(), 21);
    }

    #[test]
    fn read_only_mode_refuses_every_change_before_any_request() {
        agent_cli_core::testing::assert_read_only_refuses(&[DOMAIN]);
    }
}
