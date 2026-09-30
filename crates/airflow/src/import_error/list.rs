use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::{Airflow, At, note_more};

use super::{ImportErrorRow, import_error_row};

#[derive(clap::Args)]
pub struct ImportErrorListArgs {
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn import_error_list(ctx: &Ctx, args: ImportErrorListArgs) -> Result<Vec<ImportErrorRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    let (errors, total) = client.list(
        "importErrors",
        "order_by=-timestamp",
        "import_errors",
        args.limit,
    )?;
    note_more(ctx, errors.len(), total);
    Ok(errors.iter().map(import_error_row).collect())
}

command! {
    pub IMPORT_ERROR_LIST = ["airflow", "import-error", "list"], Read,
    "List DAG files that fail to import, newest first: why a DAG is missing",
    keywords: ["broken", "parse", "syntax", "missing", "dag", "file", "exception", "load"],
    example: "airflow import-error list --fields id,file,error",
    run: import_error_list,
}
