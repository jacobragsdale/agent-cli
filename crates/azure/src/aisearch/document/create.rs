//! `aisearch document create`: a batch of `upload` actions, which insert a
//! document or replace it whole.

use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, command};
use anyhow::Result;

use crate::aisearch::document::{Indexed, batch, documents};
use crate::aisearch::holder;
use crate::aisearch::index::named;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct DocumentCreateArgs {
    /// The index: SERVICE/INDEX from index list, a bare name or an alias
    index: String,
    /// The service that holds the index; needed when more than one does
    #[arg(long)]
    service: Option<String>,
    /// Documents as JSON (an array, one object, or JSON lines; at most 1000), or - to read stdin
    #[arg(long, allow_hyphen_values = true)]
    docs: Option<String>,
    /// The documents from a JSON or JSON-lines file
    #[arg(long)]
    docs_file: Option<PathBuf>,
}

fn document_create(ctx: &Ctx, args: DocumentCreateArgs) -> Result<Vec<Indexed>> {
    let items = documents(ctx, args.docs.as_deref(), args.docs_file.as_deref())?;
    let azure = Azure::load(ctx)?;
    let (service, index) = named(&args.index, args.service.as_deref())?;
    let search = holder(ctx, &azure, "indexes", &index, service.as_deref())?;
    batch(&search, &index, Effect::Write, "upload", items)
}

command! {
    pub DOCUMENT_CREATE = ["aisearch", "document", "create"], Write,
    "Upload documents to an AI Search index (insert, or replace whole by key)",
    keywords: ["upload", "insert", "push", "load", "add", "replace", "batch"],
    example: "aisearch document create srch-contoso-dev/orders --docs-file orders.jsonl --fields id,status",
    run: document_create,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, assert_dry_run};
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, admin_keys, azure};

    fn dev() -> Answer {
        testing::inventory(vec![testing::search_service(
            "srch-contoso-dev",
            "apiKeyOnly",
        )])
    }

    #[test]
    fn create_uploads_a_batch_and_a_207_fails_with_every_item_as_data() {
        let plans = assert_dry_run(
            &[AISEARCH],
            &[
                "aisearch",
                "document",
                "create",
                "srch-contoso-dev/orders",
                "--docs",
                "{\"id\": \"1\"}\n{\"id\": \"2\"}",
            ],
            vec![dev(), admin_keys()],
        );
        assert_eq!(
            plans[0]["body"],
            json!({"value": [{"id": "1", "@search.action": "upload"}, {"id": "2", "@search.action": "upload"}]})
        );
        assert_eq!(
            plans[0]["url"],
            "https://srch-contoso-dev.search.windows.net/indexes/orders/docs/index?api-version=2026-04-01"
        );

        let (outcome, _) = azure(
            &[AISEARCH],
            &["aisearch", "document", "create", "srch-contoso-dev/orders", "--docs", r#"[{"id": "1"}, {"id": "2"}]"#],
            vec![
                dev(),
                admin_keys(),
                Answer::status(207, json!({"value": [
                    {"key": "1", "status": true, "statusCode": 201},
                    {"key": "2", "status": false, "statusCode": 400, "errorMessage": "The request is invalid."}]}).to_string()),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(
            outcome.json()[1],
            json!({"id": "srch-contoso-dev/orders/2", "key": "2", "status": 400, "error": "The request is invalid."})
        );
        assert!(
            outcome.stderr.contains("1 of 2 documents failed"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = azure(
            &[AISEARCH],
            &[
                "aisearch", "document", "create", "orders", "--docs", "[1, 2]",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
