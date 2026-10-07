//! `aisearch indexer list`: `GET /indexers?$select=…` per service, then
//! each one's `/status`.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::aisearch::indexer::{failing, health, run, status};
use crate::aisearch::{Role, Search, list, reach};
use crate::client::{limited, parallel, text};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct IndexerListArgs {
    /// Search services to read (default: every one in reach)
    #[arg(long)]
    service: Vec<String>,
    /// Only indexers whose last run failed or had failed items, or whose health is error
    #[arg(long)]
    failing: bool,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IndexerRow {
    /// SERVICE/INDEXER: what indexer get, run and wait take.
    id: String,
    /// The target index's id, SERVICE/INDEX.
    index: Option<String>,
    /// The data source's name.
    source: Option<String>,
    skillset: Option<String>,
    schedule: Option<String>,
    disabled: bool,
    /// ok, error or unknown: the indexer's own health, not whether it runs now.
    health: Option<String>,
    /// The last run's: inProgress, success, transientFailure, persistentFailure or reset.
    last_status: Option<String>,
    last_start: Option<String>,
    last_end: Option<String>,
    processed: Option<u64>,
    failed: Option<u64>,
}

fn indexer_list(ctx: &Ctx, args: IndexerListArgs) -> Result<Vec<IndexerRow>> {
    let azure = Azure::load(ctx)?;
    let sessions = reach(ctx, &azure, &args.service)?
        .into_iter()
        .map(|service| Search::new(ctx, service))
        .collect::<Result<Vec<_>>>()?;
    let listed = parallel(&sessions, azure.parallel(), |search| {
        search.get(
            "/indexers?$select=name,targetIndexName,dataSourceName,skillsetName,schedule,disabled",
            Role::Definitions,
        )
    });
    let mut found: Vec<(&Search<'_>, Value)> = Vec::new();
    for (search, listed) in sessions.iter().zip(listed) {
        let mut indexers = list(&listed?["value"]).to_vec();
        indexers.sort_by_key(|indexer| text(&indexer["name"]));
        found.extend(indexers.into_iter().map(|indexer| (search, indexer)));
    }
    // Without --failing, only the rows printed need their status.
    if !args.failing {
        found = limited(ctx, found, args.limit);
    }
    let statuses = parallel(&found, azure.parallel(), |(search, indexer)| {
        status(search, &text(&indexer["name"]).unwrap_or_default())
    });
    let mut rows = Vec::new();
    for ((search, indexer), status) in found.iter().zip(statuses) {
        let status = status?;
        if args.failing && !failing(&status) {
            continue;
        }
        let last = run(&status["lastResult"]);
        rows.push(IndexerRow {
            id: format!(
                "{}/{}",
                search.name(),
                text(&indexer["name"]).unwrap_or_default()
            ),
            index: text(&indexer["targetIndexName"])
                .map(|index| format!("{}/{index}", search.name())),
            source: text(&indexer["dataSourceName"]),
            skillset: text(&indexer["skillsetName"]),
            schedule: text(&indexer["schedule"]["interval"]),
            disabled: indexer["disabled"].as_bool().unwrap_or(false),
            health: health(&status),
            last_status: last.as_ref().and_then(|last| last.status.clone()),
            last_start: last.as_ref().and_then(|last| last.start.clone()),
            last_end: last.as_ref().and_then(|last| last.end.clone()),
            processed: last.as_ref().and_then(|last| last.processed),
            failed: last.as_ref().and_then(|last| last.failed),
        });
    }
    if args.failing {
        rows = limited(ctx, rows, args.limit);
    }
    Ok(rows)
}

command! {
    pub INDEXER_LIST = ["aisearch", "indexer", "list"], Read,
    "List AI Search indexers with their last run: status, items processed and failed",
    keywords: ["indexing", "failing", "failed", "errors", "health", "schedule", "datasource", "sync"],
    example: "aisearch indexer list --failing --fields id,last_status,failed",
    run: indexer_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, azure};

    #[test]
    fn failing_keeps_a_success_with_failed_items_and_running_prints_as_ok() {
        let (outcome, transport) = azure(
            &[AISEARCH],
            &["aisearch", "indexer", "list", "--failing"],
            vec![
                testing::inventory(vec![testing::search_service(
                    "srch-contoso-prod",
                    "aadOrApiKey",
                )]),
                Answer::json(&json!({"value": [
                    {"name": "orders-sql", "targetIndexName": "orders", "dataSourceName": "orders-sql", "schedule": {"interval": "PT1H"}, "disabled": false},
                    {"name": "products-blob", "targetIndexName": "products", "dataSourceName": "products-blob", "disabled": false}]})),
                Answer::json(
                    &json!({"status": "running", "lastResult": {"status": "success", "itemsProcessed": 412, "itemsFailed": 1,
                    "startTime": "2026-09-29T00:40:00.123Z", "endTime": "2026-09-29T00:41:12.456Z"}}),
                ),
                Answer::json(
                    &json!({"status": "running", "lastResult": {"status": "success", "itemsProcessed": 9, "itemsFailed": 0}}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "srch-contoso-prod/orders-sql", "index": "srch-contoso-prod/orders", "source": "orders-sql",
                "schedule": "PT1H", "disabled": false, "health": "ok", "last_status": "success",
                "last_start": "2026-09-29T00:40:00Z", "last_end": "2026-09-29T00:41:12Z", "processed": 412, "failed": 1}])
        );
        assert_eq!(
            transport.sent()[2].url,
            "https://srch-contoso-prod.search.windows.net/indexers/orders-sql/status?api-version=2026-04-01"
        );
    }
}
