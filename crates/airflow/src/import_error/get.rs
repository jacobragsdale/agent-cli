use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Airflow, At, text};
use crate::source::{failing_line, repo_file};

use super::import_error_row;

#[derive(clap::Args)]
pub struct ImportErrorGetArgs {
    /// The import error's id, from import-error list
    id: i64,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ImportErrorDetail {
    id: i64,
    file: Option<String>,
    bundle: Option<String>,
    timestamp: Option<String>,
    error: Option<String>,
    /// The line in `file` the import failed at: the trace's last frame there.
    line: Option<u64>,
    /// REPO:PATH[:LINE] in Azure DevOps, which ado file get takes (when the
    /// instance names its dags_repo). A file that fails to import has no DAG,
    /// so source get cannot show it.
    #[serde(skip_serializing_if = "Option::is_none")]
    repo_file: Option<String>,
    stack_trace: Option<String>,
}

fn import_error_get(ctx: &Ctx, args: ImportErrorGetArgs) -> Result<ImportErrorDetail> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    let error = client.get(&format!("importErrors/{}", args.id))?;
    let row = import_error_row(&error);
    let stack_trace = text(&error["stack_trace"]);
    let line = row
        .file
        .as_deref()
        .zip(stack_trace.as_deref())
        .and_then(|(file, trace)| failing_line(trace.lines(), file));
    let repo_file = row
        .file
        .as_deref()
        .and_then(|file| repo_file(client.instance, file))
        .map(|repo_file| match line {
            Some(line) => format!("{repo_file}:{line}"),
            None => repo_file,
        });
    if let Some(repo_file) = &repo_file {
        ctx.note(format!("[next: agent-cli ado file get {repo_file}]"));
    }
    Ok(ImportErrorDetail {
        id: row.id,
        file: row.file,
        bundle: row.bundle,
        timestamp: row.timestamp,
        error: row.error,
        line,
        repo_file,
        stack_trace,
    })
}

command! {
    pub IMPORT_ERROR_GET = ["airflow", "import-error", "get"], Read,
    "Show an import error's full stack trace, its line and that file in the repo",
    keywords: ["trace", "traceback", "full", "broken", "dag", "file", "line", "repo"],
    example: "airflow import-error get 12 --fields error,line,repo_file",
    run: import_error_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CONFIG, airflow, airflow_with};

    #[test]
    fn an_import_error_names_its_line_and_the_file_in_the_dags_repo() {
        let trace = "Traceback (most recent call last):\n  File \"<frozen importlib._bootstrap>\", line 488, in _call_with_frames_removed\n  File \"/opt/airflow/dags/customer_sync.py\", line 5, in <module>\n    from contoso_crm import Client\nModuleNotFoundError: No module named 'contoso_crm'";
        let error = json!({"import_error_id": 12, "filename": "customer_sync.py",
            "bundle_name": "dags-folder", "stack_trace": trace});
        let config = CONFIG.replace("k8s_scope", "dags_repo = \"airflow-dags:dags\"\nk8s_scope");
        let (outcome, _) = airflow_with(
            &config,
            &["airflow", "import-error", "get", "12"],
            vec![Answer::json(&error)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["line"], 5);
        assert_eq!(got["repo_file"], "airflow-dags:dags/customer_sync.py:5");
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli ado file get airflow-dags:dags/customer_sync.py:5]"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = airflow(
            &["airflow", "import-error", "get", "12"],
            vec![Answer::json(&error)],
        );
        assert_eq!(outcome.json()["line"], 5);
        assert_eq!(outcome.json().get("repo_file"), None, "no dags_repo");
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);
    }
}
