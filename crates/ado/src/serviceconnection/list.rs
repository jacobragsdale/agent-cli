use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, list, query_value, text};

use super::parameter;

#[derive(clap::Args)]
pub struct ServiceconnectionListArgs {
    /// Only names containing this, in any case
    name: Option<String>,
    /// Only connections of this type: azurerm, dockerregistry, github, generic …
    #[arg(long = "type")]
    kind: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceConnectionRow {
    /// What serviceconnection get takes.
    id: String,
    name: Option<String>,
    /// azurerm, dockerregistry, github, generic …
    #[serde(rename = "type")]
    kind: Option<String>,
    /// ServicePrincipal, WorkloadIdentityFederation, ManagedServiceIdentity,
    /// UsernamePassword …
    scheme: Option<String>,
    /// The Entra app's client id: what entra app get takes.
    app_id: Option<String>,
    tenant_id: Option<String>,
    /// The Azure subscription's id.
    subscription: Option<String>,
    ready: Option<bool>,
    shared: Option<bool>,
}

fn serviceconnection_list(
    ctx: &Ctx,
    args: ServiceconnectionListArgs,
) -> Result<Vec<ServiceConnectionRow>> {
    let ado = Ado::load(ctx)?;
    let query = args
        .kind
        .as_deref()
        .map(|kind| format!("type={}", query_value(kind.trim())))
        .unwrap_or_default();
    // The docs' samples show authorization.parameters in list answers
    // (includeDetails is internal only). VERIFY(work): an azurerm one
    // carries tenantid and serviceprincipalid there, as get's does.
    let answer = ado.get(ctx, &ado.code("serviceendpoint/endpoints", &query))?;
    let wanted = args.name.as_deref().map(|name| name.trim().to_lowercase());
    let mut rows: Vec<ServiceConnectionRow> = list(&answer["value"])
        .iter()
        .filter(|endpoint| match &wanted {
            Some(wanted) => endpoint["name"]
                .as_str()
                .is_some_and(|name| name.to_lowercase().contains(wanted)),
            None => true,
        })
        .filter_map(|endpoint| {
            Some(ServiceConnectionRow {
                id: text(&endpoint["id"])?,
                name: text(&endpoint["name"]),
                kind: text(&endpoint["type"]),
                scheme: text(&endpoint["authorization"]["scheme"]),
                app_id: parameter(endpoint, "serviceprincipalid"),
                tenant_id: parameter(endpoint, "tenantid"),
                subscription: text(&endpoint["data"]["subscriptionId"]),
                ready: endpoint["isReady"].as_bool(),
                shared: endpoint["isShared"].as_bool(),
            })
        })
        .collect();
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!(
            "[first {}; --limit N, or narrow with NAME or --type]",
            args.limit
        ));
    }
    Ok(rows)
}

command! {
    pub SERVICECONNECTION_LIST = ["ado", "serviceconnection", "list"], Read,
    "List service connections pipelines deploy with, each with its Entra app id",
    keywords: ["endpoints", "azurerm", "spn", "credentials", "all"],
    example: "ado serviceconnection list --type azurerm --fields id,name,scheme,app_id",
    run: serviceconnection_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{CODE, ado, page, urls};

    fn prod() -> serde_json::Value {
        json!({"id": "00000000-0000-4000-8000-0000000ad5c1", "name": "contoso-prod",
            "type": "azurerm", "url": "https://management.azure.com/", "isReady": true, "isShared": false,
            "data": {"subscriptionId": "00000000-0000-4000-8000-000000005b01", "subscriptionName": "contoso-prod"},
            "authorization": {"scheme": "ServicePrincipal", "parameters": {
                "tenantid": "00000000-0000-4000-8000-00000000a001",
                "serviceprincipalid": "00000000-0000-4000-8000-000000005d01",
                "authenticationType": "spnKey", "serviceprincipalkey": "hunter2-not-masked"}}})
    }

    #[test]
    fn service_connections_list_with_their_app_id_and_never_a_secret() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "serviceconnection",
                "list",
                "PROD",
                "--type",
                "azurerm",
            ],
            vec![page(vec![
                prod(),
                json!({"id": "00000000-0000-4000-8000-0000000ad5c2", "name": "contoso-dev", "type": "azurerm"}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "00000000-0000-4000-8000-0000000ad5c1", "name": "contoso-prod",
                "type": "azurerm", "scheme": "ServicePrincipal",
                "app_id": "00000000-0000-4000-8000-000000005d01",
                "tenant_id": "00000000-0000-4000-8000-00000000a001",
                "subscription": "00000000-0000-4000-8000-000000005b01",
                "ready": true, "shared": false}])
        );
        assert!(!outcome.stdout.contains("hunter2"), "{}", outcome.stdout);
        assert_eq!(
            urls(&transport),
            [format!(
                "{CODE}/serviceendpoint/endpoints?type=azurerm&api-version=7.1"
            )]
        );
    }

    #[test]
    fn a_cut_list_says_how_to_see_more() {
        let (outcome, _) = ado(
            &["ado", "serviceconnection", "list", "--limit", "1"],
            vec![page(vec![prod(), prod()])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json().as_array().unwrap().len(), 1);
        assert!(
            outcome.stderr.starts_with("[first 1;"),
            "{}",
            outcome.stderr
        );
    }
}
