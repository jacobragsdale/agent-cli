use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Airflow, At, text};

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
    stack_trace: Option<String>,
}

fn import_error_get(ctx: &Ctx, args: ImportErrorGetArgs) -> Result<ImportErrorDetail> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    let error = client.get(&format!("importErrors/{}", args.id))?;
    let row = import_error_row(&error);
    Ok(ImportErrorDetail {
        id: row.id,
        file: row.file,
        bundle: row.bundle,
        timestamp: row.timestamp,
        error: row.error,
        stack_trace: text(&error["stack_trace"]),
    })
}

command! {
    pub IMPORT_ERROR_GET = ["airflow", "import-error", "get"], Read,
    "Show an import error's full stack trace",
    keywords: ["trace", "traceback", "full", "broken", "dag", "file"],
    example: "airflow import-error get 12",
    run: import_error_get,
}
