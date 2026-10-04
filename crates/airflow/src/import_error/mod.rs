//! `airflow import-error`: the import errors that keep a DAG file from
//! loading.

pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{stamp, text};

#[derive(Debug, Serialize, JsonSchema)]
pub struct ImportErrorRow {
    /// What import-error get takes.
    id: i64,
    /// The DAG file that failed to parse.
    file: Option<String>,
    bundle: Option<String>,
    timestamp: Option<String>,
    /// The exception, the stack trace's last line.
    error: Option<String>,
}

fn import_error_row(error: &Value) -> ImportErrorRow {
    ImportErrorRow {
        id: error["import_error_id"].as_i64().unwrap_or_default(),
        file: text(&error["filename"]).map(|file| crate::source::in_folder(&file)),
        bundle: text(&error["bundle_name"]),
        timestamp: stamp(&error["timestamp"]),
        error: error["stack_trace"]
            .as_str()
            .and_then(|trace| {
                trace
                    .lines()
                    .rev()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
            })
            .map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{airflow, paths};

    #[test]
    fn import_errors_list_the_exception_and_get_the_whole_trace() {
        let trace = "Traceback (most recent call last):\n  File \"/opt/airflow/dags/customer_sync.py\", line 3, in <module>\n    import contoso_sdk\nModuleNotFoundError: No module named 'contoso_sdk'\n";
        let error = json!({"import_error_id": 12, "timestamp": "2026-09-29T11:58:01.5+00:00",
            "filename": "customer_sync.py", "bundle_name": "dags-folder", "stack_trace": trace});
        let (outcome, transport) = airflow(
            &["airflow", "import-error", "list"],
            vec![Answer::json(
                &json!({"import_errors": [error.clone()], "total_entries": 1}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": 12, "file": "customer_sync.py", "bundle": "dags-folder",
                "timestamp": "2026-09-29T11:58:01Z",
                "error": "ModuleNotFoundError: No module named 'contoso_sdk'"}])
        );
        assert_eq!(
            paths(&transport),
            ["importErrors?order_by=-timestamp&limit=50&offset=0"]
        );

        let (outcome, transport) = airflow(
            &["airflow", "import-error", "get", "12"],
            vec![Answer::json(&error)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome.json()["stack_trace"]
                .as_str()
                .unwrap()
                .starts_with("Traceback")
        );
        assert_eq!(paths(&transport), ["importErrors/12"]);
    }

    #[test]
    fn import_errors_on_airflow_2_name_the_file_in_its_dags_folder() {
        let error = serde_json::json!({"import_error_id": 1, "timestamp": "2026-09-29T11:58:01.5+00:00",
            "filename": "/opt/airflow/dags/team/customer_sync.py",
            "stack_trace": "Traceback (most recent call last):\n  File \"/opt/airflow/dags/team/customer_sync.py\", line 3, in <module>\nModuleNotFoundError: No module named 'contoso_sdk'"});
        let (outcome, transport) = crate::testing::airflow_v1(
            &["airflow", "import-error", "list"],
            vec![Answer::json(
                &serde_json::json!({"import_errors": [error.clone()], "total_entries": 1}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["file"], "team/customer_sync.py");
        assert_eq!(
            transport.sent()[0].url,
            "https://airflow.contoso.example/api/v1/importErrors?order_by=-timestamp&limit=50&offset=0"
        );
        let (outcome, _) = crate::testing::airflow_v1(
            &["airflow", "import-error", "get", "customer_sync"],
            vec![
                Answer::json(
                    &serde_json::json!({"import_errors": [error.clone()], "total_entries": 1}),
                ),
                Answer::json(&error),
            ],
        );
        assert_eq!(outcome.json()["line"], 3, "{outcome:?}");
    }
}
