//! `aisearch index get`: `GET /indexes/{name}` and its `/stats`.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::aisearch::index::{IndexDetail, detail, named};
use crate::aisearch::shape::scrub;
use crate::aisearch::{Role, holder, segment};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct IndexGetArgs {
    /// The index: SERVICE/INDEX from index list, a bare name, its URL or portal link
    index: String,
    /// The service that holds it; needed when more than one does
    #[arg(long)]
    service: Option<String>,
    /// Add the whole definition, every secret as "<unchanged>" (--output FILE saves it)
    #[arg(long)]
    full: bool,
}

fn index_get(ctx: &Ctx, args: IndexGetArgs) -> Result<IndexDetail> {
    let azure = Azure::load(ctx)?;
    let (service, name) = named(&args.index, args.service.as_deref())?;
    let search = holder(ctx, &azure, "indexes", &name, service.as_deref())?;
    let path = format!("/indexes/{}", segment(&name));
    let mut definition = search.get(&path, Role::Definitions)?;
    // An alias answers for its index's definition, not always for its stats.
    let stats = search.get(&format!("{path}/stats"), Role::Definitions).ok();
    let mut row = detail(search.name(), &definition, stats.as_ref());
    if let Some(object) = definition.as_object_mut() {
        object.remove("@odata.context");
    }
    scrub(&mut definition);
    ctx.save(serde_json::to_string_pretty(&definition)?.as_bytes())?;
    if args.full {
        row.definition = Some(definition);
    }
    Ok(row)
}

command! {
    pub INDEX_GET = ["aisearch", "index", "get"], Read,
    "Show an AI Search index's schema: filterable fields, vectors, semantic configs",
    keywords: ["definition", "fields", "filterable", "sortable", "facetable", "vectorizer", "analyzer", "key"],
    example: "aisearch index get srch-contoso-prod/orders --fields key,fields,semantic",
    run: index_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, azure};

    fn definition() -> serde_json::Value {
        json!({"@odata.context": "https://srch-contoso-prod.search.windows.net/$metadata#indexes/$entity",
            "@odata.etag": "\"0x8DCE0A1B2C3D4E5\"", "name": "orders",
            "fields": [{"name": "id", "type": "Edm.String", "key": true}],
            "vectorSearch": {"vectorizers": [{"name": "aoai-embed", "kind": "azureOpenAI",
                "azureOpenAIParameters": {"resourceUri": "https://aoai-contoso.openai.azure.com", "apiKey": "fixture-aoai-key"}}]}})
    }

    #[test]
    fn full_adds_the_definition_scrubbed_and_output_saves_it_for_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orders.json");
        let (outcome, transport) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "index",
                "get",
                "srch-contoso-prod/orders",
                "--full",
                "--output",
                path.to_str().unwrap(),
            ],
            vec![
                testing::inventory(vec![
                    testing::search_service("srch-contoso-prod", "aadOrApiKey"),
                    testing::search_service("srch-contoso-dev", "apiKeyOnly"),
                ]),
                Answer::json(&definition()),
                Answer::json(
                    &json!({"documentCount": 88120, "storageSize": 1, "vectorIndexSize": 2}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = outcome.json();
        assert_eq!(row["documents"], 88120);
        assert_eq!(
            row["definition"]["vectorSearch"]["vectorizers"][0]["azureOpenAIParameters"]["apiKey"],
            "<unchanged>"
        );
        assert!(!outcome.stdout.contains("fixture-aoai-key"));
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(
            saved.contains("<unchanged>") && !saved.contains("fixture-aoai-key"),
            "{saved}"
        );
        assert!(!saved.contains("@odata.context"));
        assert_eq!(
            transport.sent()[1].url,
            "https://srch-contoso-prod.search.windows.net/indexes/orders?api-version=2026-04-01"
        );
    }

    #[test]
    fn a_bare_name_held_by_two_services_is_ambiguous_and_by_none_is_not_found() {
        let two = || {
            testing::inventory(vec![
                testing::search_service("srch-contoso-dev", "aadOrApiKey"),
                testing::search_service("srch-contoso-prod", "aadOrApiKey"),
            ])
        };
        let names = |names: &[&str]| {
            Answer::json(
                &json!({"value": names.iter().map(|n| json!({"name": n})).collect::<Vec<_>>()}),
            )
        };
        let (outcome, transport) = azure(
            &[AISEARCH],
            &["aisearch", "index", "get", "orders"],
            vec![two(), names(&["orders"]), names(&["orders", "products"])],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains(
                "orders is on more than one search service (srch-contoso-dev, srch-contoso-prod)"
            ),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent()[1].url,
            "https://srch-contoso-dev.search.windows.net/indexes?$select=name&api-version=2026-04-01"
        );

        let (outcome, _) = azure(
            &[AISEARCH],
            &["aisearch", "index", "get", "orders"],
            vec![two(), names(&["products"]), names(&[])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");

        let (outcome, transport) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "index",
                "get",
                "srch-contoso-prod/orders",
                "--service",
                "srch-contoso-dev",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }
}
