//! `aisearch service list`: the inventory alone, no data-plane call.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::aisearch::reach;
use crate::aisearch::service::{ServiceRow, row};
use crate::client::limited;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct ServiceListArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn service_list(ctx: &Ctx, args: ServiceListArgs) -> Result<Vec<ServiceRow>> {
    let azure = Azure::load(ctx)?;
    let rows = reach(ctx, &azure, &[])?.iter().map(row).collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub SERVICE_LIST = ["aisearch", "service", "list"], Read,
    "List the AI Search services the az login reaches, their tier and sign-in",
    keywords: ["cognitive", "endpoint", "tier", "sku", "replicas", "partitions", "inventory"],
    example: "aisearch service list --fields id,sku,auth,endpoint",
    run: service_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, azure};

    #[test]
    fn services_come_from_the_inventory_with_the_auth_each_takes() {
        let (outcome, transport) = azure(
            &[AISEARCH],
            &["aisearch", "service", "list"],
            vec![testing::inventory(vec![
                testing::search_service("srch-contoso-dev", "apiKeyOnly"),
                testing::search_service("srch-contoso-prod", "aadOrApiKey"),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[1],
            json!({"id": "srch-contoso-prod", "endpoint": "https://srch-contoso-prod.search.windows.net",
                "sku": "standard", "replicas": 2, "partitions": 1, "status": "running", "auth": "token",
                "semantic": "standard", "network": "Enabled",
                "subscription": "00000000-0000-0000-0000-000000000001", "resource_group": "rg-contoso", "location": "eastus"})
        );
        assert_eq!(outcome.json()[0]["auth"], "key");
        assert_eq!(transport.sent().len(), 1, "no data-plane call");
    }

    #[test]
    fn a_row_without_auth_settings_is_completed_from_arm() {
        let mut bare = testing::search_service("srch-contoso-dev", "");
        bare["endpoint"] = json!("");
        let (outcome, transport) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "service",
                "list",
                "--fields",
                "id,endpoint,auth",
            ],
            vec![
                testing::inventory(vec![bare]),
                agent_cli_core::testing::Answer::json(&json!({"properties": {
                    "endpoint": "https://srch-contoso-dev.search.windows.net/",
                    "authOptions": {"aadOrApiKey": {"aadAuthFailureMode": "http403"}},
                    "disableLocalAuth": false}})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "srch-contoso-dev", "endpoint": "https://srch-contoso-dev.search.windows.net", "auth": "token"}])
        );
        assert_eq!(
            transport.sent()[1].url,
            "https://management.azure.com/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-contoso/providers/Microsoft.Search/searchServices/srch-contoso-dev?api-version=2025-05-01"
        );
    }
}
