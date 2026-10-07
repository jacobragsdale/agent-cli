//! `aisearch document get`: `GET /indexes/{i}/docs/{key}`, the key
//! percent-encoded and otherwise as given.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::aisearch::document::located;
use crate::aisearch::shape::{Bounds, bound};
use crate::aisearch::{Each, Role, each, holder, segment};
use crate::client::refused_with;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct DocumentGetArgs {
    /// Documents: SERVICE/INDEX/KEY from document list, a KEY with --index, or its URL
    #[arg(required = true)]
    document: Vec<String>,
    /// The index, for bare keys: SERVICE/INDEX or a name
    #[arg(long)]
    index: Option<String>,
    /// The service that holds the index; needed when more than one does
    #[arg(long)]
    service: Option<String>,
    /// Fields to return, comma-separated (default: every retrievable one)
    #[arg(long)]
    select: Option<String>,
    /// Print vector fields whole rather than as "[N floats]"
    #[arg(long)]
    vectors: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Document {
    /// SERVICE/INDEX/KEY.
    id: String,
    doc: Value,
}

fn document_get(ctx: &Ctx, args: DocumentGetArgs) -> Result<Each<Document>> {
    let azure = Azure::load(ctx)?;
    let mut query = String::new();
    if let Some(select) = &args.select {
        query = format!("?$select={select}");
    }
    let bounds = Bounds {
        cut: false,
        vectors: args.vectors,
    };
    each(&args.document, |raw| {
        let (service, index, key) = located(raw, args.index.as_deref(), args.service.as_deref())?;
        let search = holder(ctx, &azure, "indexes", &index, service.as_deref())?;
        let path = format!("/indexes/{}/docs/{}{query}", segment(&index), segment(&key));
        let doc = search.get(&path, Role::Query).map_err(|error| {
            if refused_with(&error) != Some(404) {
                return error;
            }
            // A missing document is most often an indexer's failed item.
            Failure::not_found(format!("no document {key} in {}/{index}", search.name()))
                .hint(format!(
                    "agent-cli aisearch indexer list --service {} --failing",
                    search.name()
                ))
                .into()
        })?;
        Ok(Document {
            id: format!("{}/{index}/{key}", search.name()),
            doc: bound(doc, bounds, &mut false),
        })
    })
}

command! {
    pub DOCUMENT_GET = ["aisearch", "document", "get"], Read,
    "Look up AI Search documents by key: is one in the index, and what it holds",
    keywords: ["lookup", "fetch", "read", "missing", "indexed", "stale", "exists", "present", "contains"],
    example: "aisearch document get srch-contoso-prod/orders/88123",
    run: document_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, azure};

    fn one() -> Answer {
        testing::inventory(vec![testing::search_service(
            "srch-contoso-prod",
            "aadOrApiKey",
        )])
    }

    #[test]
    fn a_document_prints_whole_but_its_vectors_and_a_missing_one_points_at_the_indexers() {
        let (outcome, transport) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "document",
                "get",
                "88121",
                "--index",
                "srch-contoso-prod/orders",
            ],
            vec![
                one(),
                Answer::json(
                    &json!({"id": "88121", "summary": "z".repeat(300), "summary_vector": vec![0.1; 1536]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let doc = outcome.json();
        assert_eq!(doc["id"], "srch-contoso-prod/orders/88121");
        assert_eq!(doc["doc"]["summary"].as_str().unwrap().len(), 300);
        assert_eq!(doc["doc"]["summary_vector"], "[1536 floats]");
        assert_eq!(
            transport.sent()[1].url,
            "https://srch-contoso-prod.search.windows.net/indexes/orders/docs/88121?api-version=2026-04-01"
        );

        let (outcome, _) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "document",
                "get",
                "srch-contoso-prod/orders/88123",
            ],
            vec![one(), Answer::status(404, "")],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains(
                "hint: agent-cli aisearch indexer list --service srch-contoso-prod --failing"
            ),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "document",
                "get",
                "srch-contoso-prod/orders/88121",
                "srch-contoso-prod/orders/88123",
            ],
            vec![
                one(),
                Answer::json(&json!({"id": "88121"})),
                one(),
                Answer::status(404, ""),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "srch-contoso-prod/orders/88121", "doc": {"id": "88121"}}])
        );

        let (outcome, _) = azure(
            &[AISEARCH],
            &["aisearch", "document", "get", "88121"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "a bare key needs --index: {outcome:?}");
    }
}
