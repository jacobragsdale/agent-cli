//! What an Airflow refusal means for the agent: the exit code and the hint.

use agent_cli_core::{Exit, Failure};

/// Airflow's refusals as the next step: a 401 that a fresh token did not fix
/// is the credential, a 403 the role, a redirect the `base_url`, and a 404
/// names the list to look in.
pub(crate) fn refused(error: anyhow::Error, path: &str) -> anyhow::Error {
    let mut failure = match error.downcast::<Failure>() {
        Ok(failure) => failure,
        // No answer at all: the server is down, or base_url points nowhere.
        Err(error) => {
            return Failure::new(Exit::Failed, format!("{error:#}"))
                .hint("is the Airflow API server up? `agent-cli doctor airflow` checks it")
                .into();
        }
    };
    match failure.status {
        Some(401) => {
            failure.hint = Some(
                "the Airflow credential was refused; `agent-cli doctor airflow` checks it"
                    .to_owned(),
            );
        }
        Some(403) => {
            failure.hint =
                Some("the Airflow role behind this credential lacks this permission".to_owned());
        }
        Some(status @ (300..=399 | 404 | 405))
            if status < 400 || matches!(path, "auth/token" | "version") =>
        {
            failure.exit = Exit::Setup;
            failure.hint = Some(
                "base_url is wrong (its scheme or path prefix); fix it under [[airflow.instance]]"
                    .to_owned(),
            );
        }
        Some(404) if failure.message.contains("is mapped") => {
            failure.exit = Exit::Usage;
            failure.hint = Some(
                "name the mapped task as TASK:N; its map indexes: agent-cli airflow task list DAG/RUN"
                    .to_owned(),
            );
        }
        Some(404) => {
            let list = if path.contains("/taskInstances") {
                "agent-cli airflow task list DAG/RUN"
            } else if path.contains("/dagRuns") && !failure.message.contains("Dag with dag_id") {
                "agent-cli airflow run list --dag DAG"
            } else if path.starts_with("importErrors") {
                "agent-cli airflow import-error list"
            } else {
                "agent-cli airflow dag list"
            };
            failure.hint = Some(list.to_owned());
        }
        _ => {}
    }
    failure.into()
}
