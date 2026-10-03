//! Resource Graph: one query for every vault, registry and AKS cluster the
//! login can reach, rather than one list call per provider per subscription.
//! Ported from az-tui's `graph::inventory`, with clusters and the
//! subscription added, and cached for `[azure] refresh` seconds.

use agent_cli_core::{Ctx, Failure, Request, host_under};
use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{ARM, bearer, text};
use crate::config::Azure;

const URL: &str = "https://management.azure.com/providers/Microsoft.ResourceGraph/resources?api-version=2024-04-01";

/// The three resource types the domains know, with what each needs projected
/// under one name. The sort names `id` too: a skip token paging over a
/// non-unique sort column can hand a row back twice and miss another.
const QUERY: &str = r"resources
| where type in~ ('microsoft.keyvault/vaults', 'microsoft.containerregistry/registries', 'microsoft.containerservice/managedclusters')
| project id, name, type, subscriptionId, resourceGroup, location,
          loginServer = tostring(properties.loginServer),
          vaultUri = tostring(properties.vaultUri),
          currentKubernetesVersion = tostring(properties.currentKubernetesVersion),
          kubernetesVersion = tostring(properties.kubernetesVersion),
          powerState = tostring(properties.powerState.code)
| order by name asc, id asc";

/// Resource Graph's own cap on one page.
const PAGE: usize = 1000;

/// One key vault.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct Vault {
    pub name: String,
    pub subscription: String,
    pub resource_group: String,
    pub location: String,
    /// The data-plane base, `https://kv-contoso.vault.azure.net/`.
    pub uri: String,
}

/// One container registry.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct Registry {
    pub name: String,
    /// The data-plane host, `contosoacr.azurecr.io`, which `docker pull` takes.
    pub login_server: String,
    pub subscription: String,
    pub resource_group: String,
    pub location: String,
}

/// One AKS cluster.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct Cluster {
    pub name: String,
    pub resource_group: String,
    pub subscription: String,
    pub location: String,
    /// The version it runs, else the one it was asked to.
    pub kubernetes_version: String,
    /// Running or Stopped.
    pub power_state: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Inventory {
    pub vaults: Vec<Vault>,
    pub registries: Vec<Registry>,
    pub clusters: Vec<Cluster>,
}

/// Everything the login reaches in `[azure] subscriptions` (all of them when
/// none are named), before any allowlist: from the cache when it is fresh,
/// else from Resource Graph. The cache holds names and addresses only, and
/// every address read back from it is checked again before a token goes to it.
pub fn inventory(ctx: &Ctx, azure: &Azure) -> Result<Inventory> {
    let key = format!("azure:inventory:{}", azure.subscriptions.join(","));
    if let Some(held) = ctx.cache().get::<Inventory>(&key) {
        return Ok(held);
    }
    let read = query(ctx, azure).map_err(explain)?;
    // A login with no subscription (a tenant-level one) gets an empty answer,
    // not a refusal: say so rather than report an empty Azure.
    let empty = read.vaults.is_empty() && read.registries.is_empty() && read.clusters.is_empty();
    // The check is advice: if it cannot be made, the empty answer stands.
    if empty && azure.subscriptions.is_empty() && no_subscriptions(ctx).unwrap_or(false) {
        return Err(Failure::setup("the login can see no Azure subscriptions")
            .hint(
                "az account list shows what it sees; az login to an account or tenant that has one",
            )
            .into());
    }
    ctx.cache().put(&key, &read, azure.refresh());
    Ok(read)
}

/// Whether the login sees no subscription at all (ARM `GET /subscriptions`).
fn no_subscriptions(ctx: &Ctx) -> Result<bool> {
    let url = "https://management.azure.com/subscriptions?api-version=2022-12-01";
    let answer = ctx
        .read(Request::get(url).auth(&bearer(ctx, ARM)))?
        .json()?;
    Ok(answer["value"].as_array().is_none_or(Vec::is_empty))
}

fn query(ctx: &Ctx, azure: &Azure) -> Result<Inventory> {
    // A constant, but the token's one rule is checked where it is attached.
    if !host_under(URL, "management.azure.com") {
        bail!("{URL} is not ARM; the ARM token is not sent there");
    }
    let mint = bearer(ctx, ARM);
    let mut inventory = Inventory::default();
    let mut skip: Option<String> = None;
    loop {
        let answer = ctx
            .read(Request::query(URL, body(azure, skip.as_deref())).auth(&mint))?
            .json()?;
        if !answer["data"].is_array() {
            bail!("Resource Graph answered without a data array");
        }
        for row in answer["data"].as_array().into_iter().flatten() {
            let field = |key: &str| text(&row[key]).unwrap_or_default();
            match field("type").to_ascii_lowercase().as_str() {
                "microsoft.keyvault/vaults" => inventory.vaults.push(Vault {
                    name: field("name"),
                    subscription: field("subscriptionId"),
                    resource_group: field("resourceGroup"),
                    location: field("location"),
                    uri: field("vaultUri"),
                }),
                "microsoft.containerregistry/registries" => inventory.registries.push(Registry {
                    name: field("name"),
                    login_server: field("loginServer"),
                    subscription: field("subscriptionId"),
                    resource_group: field("resourceGroup"),
                    location: field("location"),
                }),
                "microsoft.containerservice/managedclusters" => {
                    inventory.clusters.push(Cluster {
                        name: field("name"),
                        resource_group: field("resourceGroup"),
                        subscription: field("subscriptionId"),
                        location: field("location"),
                        kubernetes_version: text(&row["currentKubernetesVersion"])
                            .unwrap_or_else(|| field("kubernetesVersion")),
                        power_state: field("powerState"),
                    });
                }
                _ => {}
            }
        }
        // A truncated answer carries no skip token, so paging cannot recover
        // the rest; the lists would be short and nobody would know why.
        if answer["resultTruncated"] == json!("true") || answer["resultTruncated"] == json!(true) {
            return Err(Failure::setup(format!(
                "Resource Graph truncated the answer at {} vaults, {} registries and {} clusters",
                inventory.vaults.len(),
                inventory.registries.len(),
                inventory.clusters.len()
            ))
            .hint("name the subscriptions in [azure] subscriptions to narrow it")
            .into());
        }
        let next = text(&answer["$skipToken"]);
        // A repeated token would page for ever.
        if next.is_none() || next == skip {
            return Ok(inventory);
        }
        skip = next;
    }
}

/// `subscriptions` is left out when none are named: the query then runs over
/// every subscription the login reaches. An empty array would mean the
/// opposite and answer nothing.
fn body(azure: &Azure, skip: Option<&str>) -> Value {
    let mut body = json!({
        "query": QUERY,
        "options": { "$top": PAGE, "resultFormat": "objectArray" },
    });
    let named: Vec<&String> = azure
        .subscriptions
        .iter()
        .filter(|held| !held.trim().is_empty())
        .collect();
    if !named.is_empty() {
        body["subscriptions"] = json!(named);
    }
    if let Some(token) = skip {
        body["options"]["$skipToken"] = json!(token);
    }
    body
}

/// Resource Graph answers 403, or a 400 naming
/// `NoValidSubscriptionsInQueryRequest`, when the login has rights on none of
/// the subscriptions in scope. Both mean the same, and neither reads that way.
fn explain(error: anyhow::Error) -> anyhow::Error {
    let nothing = crate::client::refused_with(&error) == Some(403)
        || format!("{error:#}").contains("NoValidSubscriptionsInQueryRequest");
    if nothing {
        return error.context("the login can see no subscriptions in scope; run `az account list`");
    }
    error.context("reading the Resource Graph inventory")
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, FakeTransport, ctx};
    use agent_cli_core::{Exit, Setup};

    use super::*;
    use crate::testing;

    fn read(answers: Vec<Answer>, config: &str) -> (Result<Inventory>, FakeTransport) {
        let transport = FakeTransport::answering(answers);
        let ctx = ctx(Setup::fake(transport.clone()).with_config(config));
        let azure = Azure::load(&ctx).unwrap();
        (inventory(&ctx, &azure), transport)
    }

    #[test]
    fn two_pages_are_joined_and_every_kind_is_read() {
        let (inventory, transport) = read(
            vec![
                Answer::json(&json!({
                    "data": [testing::vault("kv-contoso"), testing::cluster("aks-contoso")],
                    "$skipToken": "page-2",
                })),
                testing::inventory(vec![testing::registry("contosoacr")]),
            ],
            "",
        );
        let inventory = inventory.unwrap();
        assert_eq!(
            inventory.vaults[0].uri,
            "https://kv-contoso.vault.azure.net/"
        );
        assert_eq!(
            inventory.registries[0].login_server,
            "contosoacr.azurecr.io"
        );
        assert_eq!(inventory.clusters[0].kubernetes_version, "1.30.4");
        assert_eq!(inventory.clusters[0].power_state, "Running");
        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[1].body.as_ref().unwrap()["options"]["$skipToken"],
            "page-2"
        );
        assert_eq!(
            sent[0].authorization.as_deref(),
            Some("Bearer token@https://management.azure.com/")
        );
        assert!(
            sent[0]
                .url
                .starts_with("https://management.azure.com/providers/Microsoft.ResourceGraph")
        );
    }

    #[test]
    fn subscriptions_are_named_only_when_the_config_names_them() {
        let (_, transport) = read(vec![testing::inventory(vec![])], "");
        let body = transport.sent()[0].body.clone().unwrap();
        assert!(body.get("subscriptions").is_none(), "{body}");
        assert!(
            body["query"]
                .as_str()
                .unwrap()
                .contains("order by name asc, id asc")
        );

        let (_, transport) = read(
            vec![testing::inventory(vec![])],
            "[azure]\nsubscriptions = [\"00000000-0000-0000-0000-000000000001\", \" \"]\n",
        );
        assert_eq!(
            transport.sent()[0].body.as_ref().unwrap()["subscriptions"],
            json!(["00000000-0000-0000-0000-000000000001"])
        );
    }

    #[test]
    fn a_repeated_skip_token_ends_the_paging() {
        let page = Answer::json(&json!({"data": [], "$skipToken": "same"}));
        let (inventory, transport) = read(vec![page.clone(), page], "");
        inventory.unwrap();
        // Two pages, then the check an empty answer makes for subscriptions.
        assert_eq!(transport.sent().len(), 3);
        assert!(transport.sent()[2].url.contains("/subscriptions?"));
    }

    #[test]
    fn a_login_that_sees_no_subscription_is_told_so_not_handed_an_empty_azure() {
        let none = Answer::json(&json!({"value": []}));
        let (inventory, _) = read(vec![testing::inventory(vec![]), none], "");
        let error = inventory.unwrap_err();
        assert!(format!("{error:#}").contains("the login can see no Azure subscriptions"));
        let some = Answer::json(
            &json!({"value": [{"subscriptionId": "00000000-0000-0000-0000-000000000001"}]}),
        );
        let (inventory, _) = read(vec![testing::inventory(vec![]), some], "");
        assert!(
            inventory.unwrap().vaults.is_empty(),
            "one with subscriptions and no vaults is empty"
        );
    }

    #[test]
    fn a_truncated_answer_an_empty_body_and_a_forbidden_login_are_errors() {
        let (inventory, _) = read(
            vec![Answer::json(
                &json!({"data": [testing::vault("kv-a")], "resultTruncated": "true"}),
            )],
            "",
        );
        let error = inventory.unwrap_err();
        assert!(format!("{error:#}").contains("truncated"), "{error:#}");

        let (inventory, _) = read(vec![Answer::ok("")], "");
        let error = format!("{:#}", inventory.unwrap_err());
        assert!(
            error.contains("empty body"),
            "an empty body is never zero rows: {error}"
        );

        let (inventory, _) = read(
            vec![Answer::status(
                403,
                r#"{"error":{"code":"Forbidden","message":"no rights"}}"#,
            )],
            "",
        );
        let error = inventory.unwrap_err();
        assert!(
            format!("{error:#}").contains("az account list"),
            "{error:#}"
        );
        let failure = error
            .chain()
            .find_map(|c| c.downcast_ref::<Failure>())
            .unwrap();
        assert_eq!(failure.exit, Exit::Failed);
    }
}
