use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::access::{AccessRow, rules};
use crate::kubectl::{At, items};
use crate::shape::{EventRow, events_in};

use super::{ServiceAccountRow, row};

#[derive(clap::Args)]
pub struct ServiceAccountGetArgs {
    /// The service account: its id (cluster/namespace/name), namespace/name, or name
    account: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceAccountDetail {
    #[serde(flatten)]
    account: ServiceAccountRow,
    /// What RBAC lets it do in its namespace, as `access list --as` lists it.
    can: Vec<AccessRow>,
    /// The five newest events about it, newest first.
    events: Vec<EventRow>,
}

fn serviceaccount_get(ctx: &Ctx, args: ServiceAccountGetArgs) -> Result<ServiceAccountDetail> {
    let (target, name) = args.at.named(ctx, &args.account)?;
    let item = target.json(ctx, &["get", "serviceaccount", &name, "-o", "json"])?;
    let listed = target.json(ctx, &["get", "pods,events", "-o", "json"])?;
    let pods: Vec<&Value> = items(&listed)
        .filter(|held| held["kind"] == "Pod")
        .collect();
    let account = row(&target, &item, &pods)
        .ok_or_else(|| anyhow::anyhow!("kubectl answered a service account with no name"))?;
    let namespace = target.namespace.clone().unwrap_or_default();
    let subject = format!("system:serviceaccount:{namespace}:{name}");
    Ok(ServiceAccountDetail {
        account,
        can: rules(ctx, &target, Some(&subject))?,
        events: events_in(&target, &listed, &item),
    })
}

command! {
    pub SERVICEACCOUNT_GET = ["k8s", "serviceaccount", "get"], Read,
    "Show a service account: its Azure client id, pods, what RBAC lets it do",
    keywords: ["sa", "rbac", "permissions", "can-i", "allowed", "may"],
    example: "k8s serviceaccount get prod/web/worker --fields client_id,pods,can",
    run: serviceaccount_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn a_service_account_shows_its_client_id_its_pods_and_what_it_may_do() {
        let outcome = run(&["k8s", "serviceaccount", "get", "orders-api"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let account = outcome.json();
        assert_eq!(account["id"], "qa/dev/orders-api");
        assert_eq!(account["client_id"], "00000000-0000-4000-8000-0000000000c1");
        assert_eq!(account["pods"], json!(["qa/dev/orders-api-7d9f5b-abc12"]));
        assert_eq!(
            account["can"],
            json!([{"resource": "configmaps", "names": ["orders-config"], "verbs": ["get"]},
                {"resource": "secrets", "names": ["orders-db"], "verbs": ["get"]}])
        );

        let outcome = run(&["k8s", "serviceaccount", "get", "gone"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli k8s serviceaccount list"),
            "{}",
            outcome.stderr
        );
    }
}
