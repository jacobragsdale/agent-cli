//! `aisearch document delete`: a batch of `delete` actions, one index at a
//! time. Only the key is sent, under the index's key field.

use agent_cli_core::{Ctx, Effect, Failure, command};
use anyhow::Result;
use serde_json::{Value, json};

use crate::aisearch::document::{Indexed, batch, key_name, located, schema};
use crate::aisearch::holder;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct DocumentDeleteArgs {
    /// Documents of one index: SERVICE/INDEX/KEY from document list, or KEYs with --index
    #[arg(required = true)]
    document: Vec<String>,
    /// The index, for bare keys: SERVICE/INDEX or a name
    #[arg(long)]
    index: Option<String>,
    /// The service that holds the index; needed when more than one does
    #[arg(long)]
    service: Option<String>,
}

fn document_delete(ctx: &Ctx, args: DocumentDeleteArgs) -> Result<Vec<Indexed>> {
    let mut place: Option<(Option<String>, String)> = None;
    let mut keys = Vec::new();
    for raw in &args.document {
        let (service, index, key) = located(raw, args.index.as_deref(), args.service.as_deref())?;
        match &place {
            Some(held) if *held != (service.clone(), index.clone()) => {
                return Err(Failure::usage("the documents are in more than one index")
                    .hint("delete one index's documents at a time")
                    .into());
            }
            Some(_) => {}
            None => place = Some((service, index)),
        }
        keys.push(key);
    }
    let (service, index) = place.unwrap_or_default();
    let azure = Azure::load(ctx)?;
    let search = holder(ctx, &azure, "indexes", &index, service.as_deref())?;
    let key = key_name(schema(ctx, &search, &index)?.as_ref());
    let items: Vec<Value> = keys
        .iter()
        .map(|held| json!({ key.as_str(): held }))
        .collect();
    batch(&search, &index, Effect::Destructive, "delete", items)
}

command! {
    pub DOCUMENT_DELETE = ["aisearch", "document", "delete"], Destructive,
    "Delete AI Search documents by key from one index",
    keywords: ["remove", "drop", "purge", "erase"],
    example: "aisearch document delete srch-contoso-dev/orders/88123 --yes --fields id,status",
    run: document_delete,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, assert_dry_run};
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, admin_keys, azure};

    #[test]
    fn delete_sends_each_key_under_the_key_field_of_one_index() {
        let dev = || {
            testing::inventory(vec![testing::search_service(
                "srch-contoso-dev",
                "apiKeyOnly",
            )])
        };
        let definition = Answer::json(
            &json!({"name": "orders", "fields": [{"name": "order_id", "type": "Edm.String", "key": true}]}),
        );
        let plans = assert_dry_run(
            &[AISEARCH],
            &[
                "aisearch",
                "document",
                "delete",
                "srch-contoso-dev/orders/88123",
                "88124",
                "--index",
                "srch-contoso-dev/orders",
            ],
            vec![dev(), admin_keys(), definition],
        );
        assert_eq!(
            plans[0]["body"],
            json!({"value": [{"order_id": "88123", "@search.action": "delete"}, {"order_id": "88124", "@search.action": "delete"}]})
        );
        let (outcome, transport) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "document",
                "delete",
                "srch-contoso-dev/orders/1",
                "srch-contoso-dev/products/2",
                "--yes",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }
}
