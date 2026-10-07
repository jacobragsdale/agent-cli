//! `aisearch service get`: `GET /servicestats`, the tier's usage and quotas.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::aisearch::refs::{Want, parse};
use crate::aisearch::service::{ServiceRow, row};
use crate::aisearch::{Role, Search, one};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct ServiceGetArgs {
    /// The service: its name, its endpoint or its portal link
    service: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceDetail {
    #[serde(flatten)]
    service: ServiceRow,
    usage: Usage,
    limits: Limits,
}

/// What the service holds against what its tier allows; `quota` is absent
/// where the tier sets none. Storage is in bytes.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Usage {
    documents: Option<Counter>,
    indexes: Option<Counter>,
    indexers: Option<Counter>,
    data_sources: Option<Counter>,
    skillsets: Option<Counter>,
    synonym_maps: Option<Counter>,
    aliases: Option<Counter>,
    storage: Option<Counter>,
    vector_storage: Option<Counter>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Counter {
    used: Option<u64>,
    quota: Option<u64>,
}

/// The tier's per-index limits (storage in bytes).
#[derive(Debug, Serialize, JsonSchema)]
pub struct Limits {
    fields_per_index: Option<u64>,
    storage_per_index: Option<u64>,
    field_nesting_depth: Option<u64>,
    complex_collections_per_index: Option<u64>,
    complex_objects_per_document: Option<u64>,
    /// S3 HD and serverless: indexer seconds a UTC day, shared.
    indexer_seconds_per_day: Option<u64>,
}

fn service_get(ctx: &Ctx, args: ServiceGetArgs) -> Result<ServiceDetail> {
    let azure = Azure::load(ctx)?;
    let named = parse(&args.service, Want::Service)?.service;
    let search = Search::new(ctx, one(ctx, &azure, named.as_deref())?)?;
    let stats = search.get("/servicestats", Role::Definitions)?;
    let counters = &stats["counters"];
    let counter = |key: &str| {
        let held = &counters[key];
        (!held.is_null()).then(|| Counter {
            used: held["usage"].as_u64(),
            quota: held["quota"].as_u64(),
        })
    };
    let limit = |key: &str| stats["limits"][key].as_u64();
    Ok(ServiceDetail {
        service: row(&search.service),
        usage: Usage {
            documents: counter("documentCount"),
            indexes: counter("indexesCount"),
            indexers: counter("indexersCount"),
            data_sources: counter("dataSourcesCount"),
            skillsets: counter("skillsetCount"),
            synonym_maps: counter("synonymMaps"),
            aliases: counter("aliasesCount"),
            storage: counter("storageSize"),
            vector_storage: counter("vectorIndexSize"),
        },
        limits: Limits {
            fields_per_index: limit("maxFieldsPerIndex"),
            storage_per_index: limit("maxStoragePerIndex"),
            field_nesting_depth: limit("maxFieldNestingDepthPerIndex"),
            complex_collections_per_index: limit("maxComplexCollectionFieldsPerIndex"),
            complex_objects_per_document: limit("maxComplexObjectsInCollectionsPerDocument"),
            indexer_seconds_per_day: limit("maxCumulativeIndexerRuntimeSeconds"),
        },
    })
}

command! {
    pub SERVICE_GET = ["aisearch", "service", "get"], Read,
    "Show an AI Search service's storage and object counts against its tier's quotas",
    keywords: ["stats", "statistics", "quota", "left", "remaining", "capacity", "full", "tier", "429"],
    example: "aisearch service get srch-contoso-prod --fields usage,limits",
    run: service_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, azure};

    #[test]
    fn the_stats_name_usage_against_quota_with_a_token() {
        let (outcome, transport) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "service",
                "get",
                "https://srch-contoso-prod.search.windows.net/",
            ],
            vec![
                testing::inventory(vec![
                    testing::search_service("srch-contoso-prod", "aadOrApiKey"),
                    testing::search_service("srch-contoso-dev", "apiKeyOnly"),
                ]),
                Answer::json(&json!({
                    "counters": {"documentCount": {"usage": 88120}, "indexesCount": {"usage": 1, "quota": 50},
                        "storageSize": {"usage": 123456, "quota": 171798691840_u64}},
                    "limits": {"maxFieldsPerIndex": 1000, "maxStoragePerIndex": 171798691840_u64},
                })),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let detail = outcome.json();
        assert_eq!(detail["id"], "srch-contoso-prod");
        assert_eq!(detail["usage"]["documents"], json!({"used": 88120}));
        assert_eq!(detail["usage"]["indexes"], json!({"used": 1, "quota": 50}));
        assert_eq!(detail["limits"]["fields_per_index"], 1000);
        let sent = &transport.sent()[1];
        assert_eq!(
            sent.url,
            "https://srch-contoso-prod.search.windows.net/servicestats?api-version=2026-04-01"
        );
        assert_eq!(
            sent.authorization.as_deref(),
            Some("Bearer token@https://search.azure.com")
        );
        assert!(
            sent.headers.iter().all(|(name, _)| name != "api-key"),
            "never both"
        );
    }
}
