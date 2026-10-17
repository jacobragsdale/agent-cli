//! `entra app get`: one app registration with its credentials, owners,
//! federated credentials (a k8s serviceaccount or an ADO service
//! connection's subject), its service principal, and the permissions it
//! asks for, named from each API's roles and scopes. What was consented is
//! `entra serviceprincipal get`'s.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::arm::identity::{Federated, federated};
use crate::client::{parse_stamp, stamp, text};
use crate::entra::{Credential, Graph, Kind, Owner, V1, credentials, find, owners, resource};

const SELECT: &str = "id,appId,displayName,signInAudience,createdDateTime,passwordCredentials,keyCredentials,requiredResourceAccess";
/// A credential ending within this many days asks for attention.
const SOON: i64 = 30;

#[derive(clap::Args)]
pub struct AppGetArgs {
    /// The app registration: object id, app (client) id, display name, or portal link
    app: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AppDetail {
    /// The object id.
    id: String,
    /// The client id it signs in as.
    app_id: Option<String>,
    name: Option<String>,
    /// Who may sign in: AzureADMyOrg (this tenant), AzureADMultipleOrgs …
    audience: Option<String>,
    created: Option<String>,
    /// Its service principal's object id: what RBAC, ACLs and arm role list --principal take.
    service_principal: Option<String>,
    credentials: Vec<Credential>,
    federated_credentials: Vec<Federated>,
    owners: Vec<Owner>,
    /// What it asks for (consented or not): each API's role (application) or scope (delegated).
    permissions: Vec<Permission>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Permission {
    /// The API: Microsoft Graph …
    resource: String,
    name: String,
    /// application or delegated.
    kind: String,
}

/// What `requiredResourceAccess` asks for, each named from its API's
/// `appRoles` (`Role`) or `oauth2PermissionScopes` (`Scope`).
fn permissions(graph: &Graph<'_>, app: &Value) -> Result<Vec<Permission>> {
    let mut found = Vec::new();
    for asked in app["requiredResourceAccess"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(api) = text(&asked["resourceAppId"]) else {
            continue;
        };
        let held = resource(graph, &format!("(appId='{api}')"))?.unwrap_or_default();
        let api_name = text(&held["displayName"]).unwrap_or_else(|| api.clone());
        for access in asked["resourceAccess"].as_array().into_iter().flatten() {
            let id = text(&access["id"]).unwrap_or_default();
            let (list, kind) = match access["type"].as_str() {
                Some("Role") => ("appRoles", "application"),
                _ => ("oauth2PermissionScopes", "delegated"),
            };
            let name = held[list]
                .as_array()
                .into_iter()
                .flatten()
                .find(|defined| text(&defined["id"]).as_deref() == Some(id.as_str()))
                .and_then(|defined| text(&defined["value"]))
                .unwrap_or(id);
            found.push(Permission {
                resource: api_name.clone(),
                name,
                kind: kind.to_owned(),
            });
        }
    }
    Ok(found)
}

fn app_get(ctx: &Ctx, args: AppGetArgs) -> Result<AppDetail> {
    let graph = Graph::new(ctx);
    let (_, app) = find(&graph, &args.app, &[Kind::App], SELECT)?;
    let id = text(&app["id"]).unwrap_or_default();
    let app_id = text(&app["appId"]);
    let owners = owners(&graph, Kind::App, &id)?;
    let fics = graph.pages(
        &format!("{V1}/applications/{id}/federatedIdentityCredentials?$select=name,issuer,subject"),
        usize::MAX,
    )?;
    let federated_credentials = federated(ctx, &fics.rows)?;
    let service_principal = match &app_id {
        Some(app_id) => match graph.get(&format!(
            "{V1}/servicePrincipals(appId='{app_id}')?$select=id"
        )) {
            Ok(held) => text(&held["id"]),
            Err(error) if agent_cli_core::status_of(&error) == Some(404) => None,
            Err(error) => return Err(error),
        },
        None => None,
    };
    let permissions = permissions(&graph, &app)?;
    let credentials = credentials(&app);
    let soon = agent_cli_core::now() + time::Duration::days(SOON);
    let attention = credentials.iter().any(|held| {
        held.expired
            || held
                .ends
                .as_deref()
                .and_then(parse_stamp)
                .is_some_and(|ends| ends <= soon)
    });
    if attention {
        match owners
            .iter()
            .find(|owner| owner.enabled == Some(true) && owner.upn.is_some())
        {
            Some(owner) => ctx.note(format!("[next: agent-cli entra user get {}]", owner.id)),
            None => ctx.note(format!(
                "[next: agent-cli entra audit list --target {id} --since 30d]"
            )),
        }
    }
    Ok(AppDetail {
        app_id,
        name: text(&app["displayName"]),
        audience: text(&app["signInAudience"]),
        created: stamp(&app["createdDateTime"]),
        service_principal,
        credentials,
        federated_credentials,
        owners,
        permissions,
        id,
    })
}

command! {
    pub APP_GET = ["entra", "app", "get"], Read,
    "Show an Entra app registration: credentials (no values), owners, federation",
    keywords: ["registration", "client", "certificate", "expired", "federated", "workload", "permissions", "api", "owner", "owns"],
    example: "entra app get contoso-ado-deploy --fields app_id,credentials,owners",
    run: app_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::entra::testing::{V1, entra, not_found, page, urls};

    const APP: &str = "00000000-0000-4000-8000-000000005d02";
    const CLIENT: &str = "00000000-0000-4000-8000-000000005d01";
    const DANA: &str = "00000000-0000-4000-8000-00000000e101";
    const GRAPH_API: &str = "00000003-0000-0000-c000-000000000000";

    #[test]
    fn an_expired_secret_names_its_owner_next_and_permissions_are_named_from_the_api() {
        let (outcome, transport) = entra(
            &["entra", "app", "get", CLIENT],
            vec![
                not_found(),
                Answer::json(
                    &json!({"id": APP, "appId": CLIENT, "displayName": "contoso-ado-deploy",
                    "signInAudience": "AzureADMyOrg", "createdDateTime": "2025-03-17T10:00:00Z",
                    "passwordCredentials": [
                        {"keyId": "k-old", "displayName": "sc-contoso-prod", "hint": "Xy~", "secretText": null,
                         "startDateTime": "2025-03-20T00:00:00Z", "endDateTime": "2026-01-20T00:00:00Z"},
                        {"keyId": "k-new", "displayName": "sc-contoso-prod-2026", "hint": "Qw8",
                         "startDateTime": "2026-01-18T15:20:41Z", "endDateTime": "2099-03-17T00:00:00Z"}],
                    "keyCredentials": [],
                    "requiredResourceAccess": [{"resourceAppId": GRAPH_API, "resourceAccess": [
                        {"id": "e1fe6dd8-ba31-4d61-89e7-88639da4683d", "type": "Scope"}]}]}),
                ),
                page(vec![
                    json!({"@odata.type": "#microsoft.graph.user", "id": DANA, "displayName": "Dana Reyes",
                    "userPrincipalName": "dana@contoso.example", "accountEnabled": true}),
                ]),
                page(vec![
                    json!({"name": "sc-contoso-dev", "issuer": "https://vstoken.dev.azure.com/o",
                    "subject": "sc://contoso/Fabrikam/contoso-dev"}),
                ]),
                Answer::json(&json!({"id": "00000000-0000-4000-8000-000000005d03"})),
                Answer::json(
                    &json!({"id": "g", "appId": GRAPH_API, "displayName": "Microsoft Graph",
                    "appRoles": [], "oauth2PermissionScopes": [
                        {"id": "e1fe6dd8-ba31-4d61-89e7-88639da4683d", "value": "User.Read"}]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["service_principal"],
            "00000000-0000-4000-8000-000000005d03"
        );
        assert_eq!(
            got["credentials"],
            json!([
                {"kind": "secret", "key_id": "k-old", "name": "sc-contoso-prod",
                 "starts": "2025-03-20T00:00:00Z", "ends": "2026-01-20T00:00:00Z", "expired": true},
                {"kind": "secret", "key_id": "k-new", "name": "sc-contoso-prod-2026",
                 "starts": "2026-01-18T15:20:41Z", "ends": "2099-03-17T00:00:00Z", "expired": false},
            ])
        );
        assert_eq!(
            got["permissions"],
            json!([{"resource": "Microsoft Graph", "name": "User.Read", "kind": "delegated"}])
        );
        assert_eq!(
            got["federated_credentials"][0]["subject"],
            "sc://contoso/Fabrikam/contoso-dev"
        );
        assert!(
            !outcome.stdout.contains("Xy~"),
            "no hint: {}",
            outcome.stdout
        );
        assert!(
            outcome
                .stderr
                .contains(&format!("[next: agent-cli entra user get {DANA}]")),
            "{}",
            outcome.stderr
        );
        let sent = urls(&transport);
        assert!(
            sent[1].starts_with(&format!("{V1}/applications(appId='{CLIENT}')?$select=")),
            "{sent:?}"
        );
        assert_eq!(
            sent[5],
            format!(
                "{V1}/servicePrincipals(appId='{GRAPH_API}')?$select=id,appId,displayName,appRoles,oauth2PermissionScopes"
            )
        );
    }
}
