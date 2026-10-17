use agent_cli_core::{Ctx, Failure, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{API, Ado, Body, list, query_value, text};
use crate::ids::{arg, is_guid, query_param, web};

use super::parameter;

#[derive(clap::Args)]
pub struct ServiceconnectionGetArgs {
    /// The connection's id, name or web URL (…/_settings/adminservices?resourceId=ID)
    connection: String,
}

/// A service connection and the Entra app, service principal and
/// subscription behind it. No field holds its key, password or token.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceConnection {
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
    subscription_name: Option<String>,
    ready: Option<bool>,
    shared: Option<bool>,
    /// The service principal's object id: what role assignments and
    /// arm role list --principal take.
    service_principal: Option<String>,
    /// The Entra app registration's object id.
    app_object: Option<String>,
    /// Workload identity federation: the token issuer and subject the app's
    /// federated credential must name.
    issuer: Option<String>,
    subject: Option<String>,
    /// Automatic (Azure DevOps made the app) or Manual.
    creation_mode: Option<String>,
    description: Option<String>,
    created_by: Option<String>,
    url: Option<String>,
    /// The projects it is shared with.
    projects: Vec<String>,
}

fn serviceconnection_get(ctx: &Ctx, args: ServiceconnectionGetArgs) -> Result<ServiceConnection> {
    let ado = Ado::load(ctx)?;
    let endpoint = find(ctx, &ado, args.connection.trim())?;
    let data = &endpoint["data"];
    Ok(ServiceConnection {
        id: text(&endpoint["id"]).unwrap_or_default(),
        name: text(&endpoint["name"]),
        kind: text(&endpoint["type"]),
        scheme: text(&endpoint["authorization"]["scheme"]),
        app_id: parameter(&endpoint, "serviceprincipalid"),
        tenant_id: parameter(&endpoint, "tenantid"),
        subscription: text(&data["subscriptionId"]),
        subscription_name: text(&data["subscriptionName"]),
        ready: endpoint["isReady"].as_bool(),
        shared: endpoint["isShared"].as_bool(),
        service_principal: text(&data["spnObjectId"]),
        app_object: text(&data["appObjectId"]),
        issuer: parameter(&endpoint, "workloadIdentityFederationIssuer"),
        subject: parameter(&endpoint, "workloadIdentityFederationSubject"),
        creation_mode: text(&data["creationMode"]),
        description: text(&endpoint["description"]),
        created_by: text(&endpoint["createdBy"]["uniqueName"]),
        url: text(&endpoint["url"]),
        projects: list(&endpoint["serviceEndpointProjectReferences"])
            .iter()
            .filter_map(|reference| text(&reference["projectReference"]["name"]))
            .collect(),
    })
}

/// The endpoint `raw` names: by GUID or web URL one GET, by name the list
/// narrowed to it (`endpointNames=`), matched exactly in any case.
fn find(ctx: &Ctx, ado: &Ado, raw: &str) -> Result<Value> {
    let mut project = ado.code_project.clone();
    let mut id = is_guid(raw).then(|| raw.to_owned());
    if let Some((segments, query)) = web(ado, raw).map_err(Failure::usage)? {
        let found = query_param(&query, "resourceId").filter(|id| is_guid(id));
        let Some(found) = found.filter(|_| segments.iter().any(|s| s == "adminservices")) else {
            return Err(
                Failure::usage(format!("{raw} is not a service connection's URL"))
                    .hint("agent-cli ado serviceconnection list --fields id,name")
                    .into(),
            );
        };
        if let Some(named) = segments.first().filter(|named| !named.is_empty()) {
            project.clone_from(named);
        }
        id = Some(found);
    }
    if let Some(id) = id {
        let url = ado.api(
            Some(&project),
            &format!("serviceendpoint/endpoints/{id}"),
            "",
            API,
        );
        // VERIFY(work): an id the project lacks answers 200 with no body
        // rather than a 404; both read as not found.
        let response = ado.send(ctx, Method::Get, &url, Body::None, None)?;
        let endpoint: Value = match response.body.trim() {
            "" => Value::Null,
            _ => response.json()?,
        };
        if endpoint["id"].is_string() {
            return Ok(endpoint);
        }
        return Err(Failure::not_found(format!(
            "there is no service connection {id} in {project}"
        ))
        .hint("agent-cli ado serviceconnection list --fields id,name")
        .into());
    }
    let answer = ado.get(
        ctx,
        &ado.code(
            "serviceendpoint/endpoints",
            &format!("endpointNames={}", query_value(raw)),
        ),
    )?;
    let mut named: Vec<Value> = list(&answer["value"])
        .iter()
        .filter(|endpoint| {
            endpoint["name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(raw))
        })
        .cloned()
        .collect();
    match named.len() {
        1 => Ok(named.remove(0)),
        0 => Err(Failure::not_found(format!(
            "there is no service connection {raw:?} in {project}"
        ))
        .hint(format!(
            "agent-cli ado serviceconnection list {} --fields id,name",
            arg(raw)
        ))
        .into()),
        _ => {
            let ids: Vec<String> = named.iter().filter_map(|e| text(&e["id"])).collect();
            Err(Failure::usage(format!(
                "{} service connections are named {raw:?}: {}",
                ids.len(),
                ids.join(", ")
            ))
            .hint("pass one's id: agent-cli ado serviceconnection get ID")
            .into())
        }
    }
}

command! {
    pub SERVICECONNECTION_GET = ["ado", "serviceconnection", "get"], Read,
    "Show a service connection's app id, service principal and subscription",
    keywords: ["endpoint", "spn", "workload", "identity", "federation", "object", "credential", "expiry", "client"],
    example: "ado serviceconnection get contoso-prod --fields app_id,service_principal,scheme",
    run: serviceconnection_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, page, urls};

    const DEV: &str = "00000000-0000-4000-8000-0000000ad5c2";

    fn dev() -> serde_json::Value {
        json!({"id": DEV, "name": "contoso-dev", "type": "azurerm",
            "url": "https://management.azure.com/", "description": "Deploys to dev",
            "isReady": true, "isShared": false,
            "createdBy": {"displayName": "Jane Doe", "uniqueName": "jane@contoso.com"},
            "data": {"subscriptionId": "00000000-0000-4000-8000-000000005b01",
                "subscriptionName": "contoso-prod", "environment": "AzureCloud",
                "scopeLevel": "Subscription", "creationMode": "Automatic",
                "appObjectId": "00000000-0000-4000-8000-000000005f02",
                "spnObjectId": "00000000-0000-4000-8000-000000005f03"},
            "authorization": {"scheme": "WorkloadIdentityFederation", "parameters": {
                "tenantid": "00000000-0000-4000-8000-00000000a001",
                "serviceprincipalid": "00000000-0000-4000-8000-000000005f01",
                "workloadIdentityFederationIssuer": "https://vstoken.dev.azure.com/00000000-0000-4000-8000-0000000ad001",
                "workloadIdentityFederationSubject": "sc://contoso/Fabrikam/contoso-dev",
                "serviceprincipalkey": "hunter2-not-masked", "password": "hunter3", "apitoken": "hunter4"}},
            "serviceEndpointProjectReferences": [{"projectReference": {"id": "p-1", "name": "Fabrikam"},
                "name": "contoso-dev"}]})
    }

    #[test]
    fn a_connection_by_id_names_its_app_and_principal_and_never_a_secret() {
        let (outcome, transport) = ado(
            &["ado", "serviceconnection", "get", DEV],
            vec![Answer::json(&dev())],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": DEV, "name": "contoso-dev", "type": "azurerm",
                "scheme": "WorkloadIdentityFederation",
                "app_id": "00000000-0000-4000-8000-000000005f01",
                "tenant_id": "00000000-0000-4000-8000-00000000a001",
                "subscription": "00000000-0000-4000-8000-000000005b01",
                "subscription_name": "contoso-prod", "ready": true, "shared": false,
                "service_principal": "00000000-0000-4000-8000-000000005f03",
                "app_object": "00000000-0000-4000-8000-000000005f02",
                "issuer": "https://vstoken.dev.azure.com/00000000-0000-4000-8000-0000000ad001",
                "subject": "sc://contoso/Fabrikam/contoso-dev",
                "creation_mode": "Automatic", "description": "Deploys to dev",
                "created_by": "jane@contoso.com", "url": "https://management.azure.com/",
                "projects": ["Fabrikam"]})
        );
        assert!(!outcome.stdout.contains("hunter"), "{}", outcome.stdout);
        assert_eq!(
            urls(&transport),
            [format!(
                "{CODE}/serviceendpoint/endpoints/{DEV}?api-version=7.1"
            )]
        );
    }

    #[test]
    fn a_connection_by_name_or_web_url_is_the_same_one() {
        let (outcome, transport) = ado(
            &["ado", "serviceconnection", "get", "CONTOSO-DEV"],
            vec![page(vec![dev()])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["id"], DEV);
        assert_eq!(
            urls(&transport),
            [format!(
                "{CODE}/serviceendpoint/endpoints?endpointNames=CONTOSO-DEV&api-version=7.1"
            )]
        );

        let web = format!(
            "https://dev.azure.com/contoso/Other%20Project/_settings/adminservices?resourceId={DEV}"
        );
        let (outcome, transport) = ado(
            &["ado", "serviceconnection", "get", &web],
            vec![Answer::json(&dev())],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport),
            [format!(
                "https://dev.azure.com/contoso/Other%20Project/_apis/serviceendpoint/endpoints/{DEV}?api-version=7.1"
            )]
        );

        let (outcome, _) = ado(
            &[
                "ado",
                "serviceconnection",
                "get",
                "https://dev.azure.com/contoso/Fabrikam/_build",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_missing_or_ambiguous_name_says_what_to_run() {
        let (outcome, _) = ado(
            &["ado", "serviceconnection", "get", "nope"],
            vec![page(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("serviceconnection list nope"),
            "{}",
            outcome.stderr
        );

        let other = json!({"id": "00000000-0000-4000-8000-0000000ad5c9", "name": "contoso-dev"});
        let (outcome, _) = ado(
            &["ado", "serviceconnection", "get", "contoso-dev"],
            vec![page(vec![dev(), other])],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");

        let (outcome, _) = ado(
            &["ado", "serviceconnection", "get", DEV],
            vec![Answer::ok("")],
        );
        assert_eq!(
            outcome.code, 4,
            "an empty answer is no connection: {outcome:?}"
        );
    }
}
