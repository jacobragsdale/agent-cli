use agent_cli_core::{Ctx, command};
use anyhow::Result;
use serde_json::Value;

use crate::kubectl::{At, items, limited, name_has};

use super::{ServiceAccountRow, row};

#[derive(clap::Args)]
pub struct ServiceAccountListArgs {
    /// Part of the service account's name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn serviceaccount_list(ctx: &Ctx, args: ServiceAccountListArgs) -> Result<Vec<ServiceAccountRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "serviceaccounts,pods", "-o", "json"])?;
    let (accounts, pods): (Vec<&Value>, Vec<&Value>) =
        items(&listed).partition(|item| item["kind"] == "ServiceAccount");
    let rows = accounts
        .into_iter()
        .filter(|account| name_has(account, args.name.as_deref()))
        .filter_map(|account| row(&target, account, &pods))
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub SERVICEACCOUNT_LIST = ["k8s", "serviceaccount", "list"], Read,
    "List service accounts: their Azure client id, the pods that run as each",
    keywords: ["sa", "entra", "workload"],
    example: "k8s serviceaccount list --cluster prod --fields id,client_id,pods",
    run: serviceaccount_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn service_accounts_name_their_azure_identity_and_the_pods_running_as_them() {
        let outcome = run(&["k8s", "serviceaccount", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"id": "qa/dev/orders-api", "name": "orders-api",
                "client_id": "00000000-0000-4000-8000-0000000000c1",
                "pull_secrets": ["qa/dev/acr-pull"],
                "pods": ["qa/dev/orders-api-7d9f5b-abc12"]})
        );
        assert_eq!(rows[1]["automount"], false);
        assert_eq!(
            rows[1]["pods"].as_array().unwrap().len(),
            4,
            "the pods that name none run as default"
        );
    }
}
