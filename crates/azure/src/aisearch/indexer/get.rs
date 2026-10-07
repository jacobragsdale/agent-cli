//! `aisearch indexer get`: the indexer, its `/status` and its data source.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::aisearch::holder;
use crate::aisearch::indexer::{IndexerDetail, definitions, detail, named, next_note, status};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct IndexerGetArgs {
    /// The indexer: SERVICE/INDEXER from indexer list, a bare name, its URL or portal link
    indexer: String,
    /// The service that holds it; needed when more than one does
    #[arg(long)]
    service: Option<String>,
}

fn indexer_get(ctx: &Ctx, args: IndexerGetArgs) -> Result<IndexerDetail> {
    let azure = Azure::load(ctx)?;
    let (service, name) = named(&args.indexer, args.service.as_deref())?;
    let search = holder(ctx, &azure, "indexers", &name, service.as_deref())?;
    let (indexer, source) = definitions(&search, &name)?;
    let status = status(&search, &name)?;
    let row = detail(ctx, &search, &indexer, &source, &status);
    next_note(ctx, &row);
    Ok(row)
}

command! {
    pub INDEXER_GET = ["aisearch", "indexer", "get"], Read,
    "Show an AI Search indexer's last run, its failed items' errors, and history",
    keywords: ["status", "errors", "warnings", "failed", "items", "why", "skipped", "skip", "datasource"],
    example: "aisearch indexer get srch-contoso-prod/orders-sql --fields last,errors",
    run: indexer_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, next_command};
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, azure};

    pub(crate) fn status() -> Answer {
        Answer::json(&json!({"name": "orders-sql", "status": "running",
            "lastResult": {"status": "success", "errorMessage": null, "startTime": "2026-09-29T00:40:00.183Z",
                "endTime": "2026-09-29T00:41:12.904Z", "itemsProcessed": 412, "itemsFailed": 1,
                "errors": [{"key": "88123", "statusCode": 400, "name": "DocumentExtraction.AzureSql.orders-sql",
                    "errorMessage": "The field 'customer_id' is required and was null.", "details": "order 88123 has no customer_id"}],
                "warnings": []},
            "executionHistory": [{"status": "success", "startTime": "2026-09-29T00:40:00.183Z", "endTime": "2026-09-29T00:41:12.904Z", "itemsProcessed": 412, "itemsFailed": 1}]}))
    }

    #[test]
    fn get_reads_the_source_and_status_and_points_at_the_failed_document() {
        let (outcome, transport) = azure(
            &[AISEARCH],
            &["aisearch", "indexer", "get", "srch-contoso-prod/orders-sql"],
            vec![
                testing::inventory(vec![testing::search_service(
                    "srch-contoso-prod",
                    "aadOrApiKey",
                )]),
                Answer::json(
                    &json!({"name": "orders-sql", "dataSourceName": "orders-sql", "targetIndexName": "orders", "schedule": {"interval": "PT1H"}}),
                ),
                Answer::json(
                    &json!({"name": "orders-sql", "type": "azuresql", "credentials": {"connectionString": null}, "container": {"name": "dbo.orders"}}),
                ),
                status(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = outcome.json();
        assert_eq!(row["health"], "ok");
        assert_eq!(
            row["source"],
            json!({"name": "orders-sql", "type": "azuresql", "container": "dbo.orders"})
        );
        assert_eq!(row["last"]["start"], "2026-09-29T00:40:00Z");
        assert_eq!(row["errors"][0]["key"], "88123");
        assert_eq!(
            next_command(&outcome.stderr),
            Some(vec![
                "aisearch".into(),
                "document".into(),
                "get".into(),
                "srch-contoso-prod/orders/88123".into()
            ])
        );
        assert_eq!(
            transport.sent()[2].url,
            "https://srch-contoso-prod.search.windows.net/datasources/orders-sql?api-version=2026-04-01"
        );
    }
}
