//! Scrubbed answers the kv, acr, aks and aisearch tests share.

use agent_cli_core::testing::{Answer, FakeTransport, Outcome, Sent, run};
use agent_cli_core::{Domain, Setup};
use serde_json::{Value, json};

/// One thread, so the fake's answers land in the order they were given.
pub const SERIAL: &str = "[azure]\nparallel = 1\n";

pub fn azure(domains: &[Domain], argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
    azure_with(domains, argv, answers, SERIAL)
}

pub fn azure_with(
    domains: &[Domain],
    argv: &[&str],
    answers: Vec<Answer>,
    config: &str,
) -> (Outcome, FakeTransport) {
    let transport = FakeTransport::answering(answers);
    let outcome = run(
        domains,
        argv,
        Setup::fake(transport.clone()).with_config(config),
    );
    (outcome, transport)
}

fn row(kind: &str, name: &str, extra: Value) -> Value {
    let mut row = json!({
        "id": format!("/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-contoso/providers/{kind}/{name}"),
        "name": name,
        "type": kind.to_ascii_lowercase(),
        "subscriptionId": "00000000-0000-0000-0000-000000000001",
        "resourceGroup": "rg-contoso",
        "location": "eastus",
        "loginServer": "",
        "vaultUri": "",
        "kubernetesVersion": "",
        "currentKubernetesVersion": "",
        "powerState": "",
    });
    for (key, value) in extra.as_object().into_iter().flatten() {
        row[key] = value.clone();
    }
    row
}

pub fn vault(name: &str) -> Value {
    row(
        "Microsoft.KeyVault/vaults",
        name,
        json!({"vaultUri": format!("https://{name}.vault.azure.net/")}),
    )
}

pub fn registry(name: &str) -> Value {
    row(
        "Microsoft.ContainerRegistry/registries",
        name,
        json!({"loginServer": format!("{name}.azurecr.io")}),
    )
}

pub fn cluster(name: &str) -> Value {
    row(
        "Microsoft.ContainerService/managedClusters",
        name,
        json!({"currentKubernetesVersion": "1.30.4", "kubernetesVersion": "1.30", "powerState": "Running"}),
    )
}

/// A search service whose `authOptions` is `{auth: {}}` (`aadOrApiKey` or
/// `apiKeyOnly`), or absent when `auth` is empty.
pub fn search_service(name: &str, auth: &str) -> Value {
    let options = if auth.is_empty() {
        Value::Null
    } else {
        json!({ auth: {} })
    };
    row(
        "Microsoft.Search/searchServices",
        name,
        json!({"endpoint": format!("https://{name}.search.windows.net/"), "sku": "standard",
            "replicaCount": 2, "partitionCount": 1, "status": "running",
            "publicNetworkAccess": "Enabled", "semanticSearch": "standard",
            "authOptions": options, "disableLocalAuth": if auth.is_empty() { Value::Null } else { json!(false) }}),
    )
}

/// The ARM answer to `listAdminKeys`.
pub fn admin_keys() -> Answer {
    Answer::json(
        &json!({"primaryKey": "fixture-admin-key-1", "secondaryKey": "fixture-admin-key-2"}),
    )
}

/// A context for calling a domain's doctor directly.
pub fn doctor_ctx(answers: Vec<Answer>, config: &str) -> (agent_cli_core::Ctx, FakeTransport) {
    let transport = FakeTransport::answering(answers);
    let ctx = agent_cli_core::testing::ctx(Setup::fake(transport.clone()).with_config(config));
    (ctx, transport)
}

/// Each check's name and whether it passed.
pub fn rows(checks: &[agent_cli_core::Check]) -> Vec<(String, bool)> {
    checks
        .iter()
        .map(|check| (check.check.clone(), check.ok))
        .collect()
}

/// A Resource Graph answer with `rows`, on one page.
pub fn inventory(rows: Vec<Value>) -> Answer {
    Answer::json(&json!({"totalRecords": rows.len(), "count": rows.len(), "data": rows}))
}

pub fn listing(vault: &str, names: &[&str]) -> Answer {
    Answer::json(&json!({
        "value": names.iter().map(|name| json!({
            "id": format!("https://{vault}.vault.azure.net/secrets/{name}"),
            "contentType": "text/plain",
            "tags": {"owner": "platform"},
            "attributes": {"enabled": true, "created": 1_709_251_200, "updated": 1_725_753_600},
        })).collect::<Vec<_>>(),
        "nextLink": null,
    }))
}

pub fn two_vaults() -> Answer {
    inventory(vec![vault("kv-contoso-dev"), vault("kv-contoso-prod")])
}

pub fn exchanged() -> Answer {
    Answer::json(&json!({"refresh_token": "refresh-fixture-1"}))
}

pub fn issued(token: &str) -> Answer {
    Answer::json(&json!({"access_token": token}))
}

pub fn one_registry() -> Answer {
    inventory(vec![registry("contosoacr")])
}

pub fn field<'a>(sent: &'a Sent, key: &str) -> Option<&'a str> {
    sent.body.as_ref()?[key].as_str()
}
