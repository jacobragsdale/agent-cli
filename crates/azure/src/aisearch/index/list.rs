//! `aisearch index list`: `GET /indexes?$select=…` per service (GA lists are
//! not paged), then each index's `/stats`.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::aisearch::{Role, Search, list, reach, segment};
use crate::client::{limited, parallel, text};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct IndexListArgs {
    /// Only indexes whose name holds this, ignoring case
    name: Option<String>,
    /// Search services to read (default: every one in reach)
    #[arg(long)]
    service: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IndexRow {
    /// SERVICE/INDEX: what index get and document list take.
    id: String,
    service: String,
    name: String,
    documents: Option<u64>,
    /// Bytes.
    storage: Option<u64>,
    vector_storage: Option<u64>,
    /// How many fields it has.
    fields: usize,
    vector_fields: Vec<String>,
    /// The default semantic configuration.
    semantic: Option<String>,
}

fn index_list(ctx: &Ctx, args: IndexListArgs) -> Result<Vec<IndexRow>> {
    let azure = Azure::load(ctx)?;
    let sessions = reach(ctx, &azure, &args.service)?
        .into_iter()
        .map(|service| Search::new(ctx, service))
        .collect::<Result<Vec<_>>>()?;
    let listed = parallel(&sessions, azure.parallel(), |search| {
        search.get(
            "/indexes?$select=name,fields,semantic,vectorSearch",
            Role::Definitions,
        )
    });
    let wanted = args.name.as_deref().map(str::to_ascii_lowercase);
    let mut found: Vec<(&Search<'_>, Value)> = Vec::new();
    for (search, listed) in sessions.iter().zip(listed) {
        let mut indexes: Vec<Value> = list(&listed?["value"])
            .iter()
            .filter(|index| {
                wanted.as_deref().is_none_or(|wanted| {
                    text(&index["name"])
                        .is_some_and(|name| name.to_ascii_lowercase().contains(wanted))
                })
            })
            .cloned()
            .collect();
        indexes.sort_by_key(|index| text(&index["name"]));
        found.extend(indexes.into_iter().map(|index| (search, index)));
    }
    // ponytail: one stats call per index, kept to the rows printed, until
    // `GET /indexstats` leaves preview.
    let found = limited(ctx, found, args.limit);
    let stats = parallel(&found, azure.parallel(), |(search, index)| {
        let name = text(&index["name"]).unwrap_or_default();
        search.get(
            &format!("/indexes/{}/stats", segment(&name)),
            Role::Definitions,
        )
    });
    found
        .iter()
        .zip(stats)
        .map(|((search, index), stats)| {
            let stats = stats?;
            let name = text(&index["name"]).unwrap_or_default();
            Ok(IndexRow {
                id: format!("{}/{name}", search.name()),
                service: search.name().to_owned(),
                documents: stats["documentCount"].as_u64(),
                storage: stats["storageSize"].as_u64(),
                vector_storage: stats["vectorIndexSize"].as_u64(),
                fields: list(&index["fields"]).len(),
                vector_fields: list(&index["fields"])
                    .iter()
                    .filter(|field| !field["vectorSearchProfile"].is_null())
                    .filter_map(|field| text(&field["name"]))
                    .collect(),
                semantic: text(&index["semantic"]["defaultConfiguration"]),
                name,
            })
        })
        .collect()
}

command! {
    pub INDEX_LIST = ["aisearch", "index", "list"], Read,
    "List AI Search indexes with their document counts, storage and vector fields",
    keywords: ["indexes", "count", "size", "storage", "vectors", "semantic", "stats"],
    example: "aisearch index list --fields id,documents,storage,vector_fields",
    run: index_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, admin_keys, azure};

    #[test]
    fn indexes_of_every_service_come_with_their_stats_and_each_service_its_own_auth() {
        let (outcome, transport) = azure(
            &[AISEARCH],
            &["aisearch", "index", "list"],
            vec![
                testing::inventory(vec![
                    testing::search_service("srch-contoso-dev", "apiKeyOnly"),
                    testing::search_service("srch-contoso-prod", "aadOrApiKey"),
                ]),
                admin_keys(),
                Answer::json(&json!({"value": [{"name": "products", "fields": [{"name": "id"}]}]})),
                Answer::json(&json!({"value": [{"name": "orders",
                    "fields": [{"name": "id"}, {"name": "summary_vector", "vectorSearchProfile": "orders-hnsw"}],
                    "semantic": {"defaultConfiguration": "orders-semantic"}}]})),
                Answer::json(
                    &json!({"documentCount": 1200, "storageSize": 2048, "vectorIndexSize": 0}),
                ),
                Answer::json(
                    &json!({"documentCount": 88120, "storageSize": 734003200, "vectorIndexSize": 541065216}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[1],
            json!({"id": "srch-contoso-prod/orders", "service": "srch-contoso-prod", "name": "orders",
                "documents": 88120, "storage": 734003200, "vector_storage": 541065216, "fields": 2,
                "vector_fields": ["summary_vector"], "semantic": "orders-semantic"})
        );
        let sent = transport.sent();
        assert!(
            sent[1]
                .url
                .ends_with("/srch-contoso-dev/listAdminKeys?api-version=2025-05-01")
        );
        assert_eq!(
            sent[2].url,
            "https://srch-contoso-dev.search.windows.net/indexes?$select=name,fields,semantic,vectorSearch&api-version=2026-04-01"
        );
        assert_eq!(sent[2].authorization, None, "a key, never both");
        assert!(
            sent[2]
                .headers
                .contains(&("api-key".to_owned(), "fixture-admin-key-1".to_owned()))
        );
        assert_eq!(
            sent[3].authorization.as_deref(),
            Some("Bearer token@https://search.azure.com")
        );
        assert!(sent[3].headers.is_empty());
        assert_eq!(
            sent[5].url,
            "https://srch-contoso-prod.search.windows.net/indexes/orders/stats?api-version=2026-04-01"
        );
    }
}
