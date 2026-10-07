//! `aisearch document update`: a batch of `merge` actions (or
//! `mergeOrUpload`), which change only the fields given.

use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, command};
use anyhow::Result;

use crate::aisearch::document::{Indexed, batch, documents};
use crate::aisearch::holder;
use crate::aisearch::index::named;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct DocumentUpdateArgs {
    /// The index: SERVICE/INDEX from index list, a bare name or an alias
    index: String,
    /// The service that holds the index; needed when more than one does
    #[arg(long)]
    service: Option<String>,
    /// The key and the fields to change, as JSON (an array, one object, or JSON lines), or - to read stdin
    #[arg(long, allow_hyphen_values = true)]
    docs: Option<String>,
    /// The documents from a JSON or JSON-lines file
    #[arg(long)]
    docs_file: Option<PathBuf>,
    /// Upload a document that is not there yet, rather than fail it (404)
    #[arg(long)]
    upsert: bool,
}

fn document_update(ctx: &Ctx, args: DocumentUpdateArgs) -> Result<Vec<Indexed>> {
    let items = documents(ctx, args.docs.as_deref(), args.docs_file.as_deref())?;
    let azure = Azure::load(ctx)?;
    let (service, index) = named(&args.index, args.service.as_deref())?;
    let search = holder(ctx, &azure, "indexes", &index, service.as_deref())?;
    let action = if args.upsert {
        "mergeOrUpload"
    } else {
        "merge"
    };
    batch(&search, &index, Effect::Write, action, items)
}

command! {
    pub DOCUMENT_UPDATE = ["aisearch", "document", "update"], Write,
    "Change fields of AI Search documents by key (merge; --upsert adds missing ones)",
    keywords: ["merge", "patch", "modify", "edit", "set", "field", "upsert", "fix", "correct"],
    example: "aisearch document update srch-contoso-dev/orders --docs '{\"id\": \"88123\", \"status\": \"fixed\"}' --fields id,status",
    run: document_update,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::assert_dry_run;

    use crate::AISEARCH;
    use crate::testing::{self, admin_keys};

    #[test]
    fn update_merges_and_upsert_merges_or_uploads() {
        let dev = || {
            testing::inventory(vec![testing::search_service(
                "srch-contoso-dev",
                "apiKeyOnly",
            )])
        };
        for (flag, action) in [(None, "merge"), (Some("--upsert"), "mergeOrUpload")] {
            let mut argv = vec![
                "aisearch",
                "document",
                "update",
                "srch-contoso-dev/orders",
                "--docs",
                r#"{"id": "88123", "customer_id": "c-17"}"#,
            ];
            argv.extend(flag);
            let plans = assert_dry_run(&[AISEARCH], &argv, vec![dev(), admin_keys()]);
            assert_eq!(plans[0]["body"]["value"][0]["@search.action"], action);
            assert_eq!(plans[0]["body"]["value"][0]["customer_id"], "c-17");
        }
    }
}
