//! `entra serviceprincipal get`: a service principal or managed identity
//! by object id, client id, ARM id or name, with the app roles and delegated
//! scopes it was granted (what `app get` only asks for), its groups and
//! owners. Azure RBAC stays `arm role list`'s.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::text;
use crate::entra::{
    Credential, Graph, Kind, Named, Owner, V1, arm_id, credentials, find, named, owners, resource,
};

const SELECT: &str = "id,appId,displayName,servicePrincipalType,accountEnabled,alternativeNames,appOwnerOrganizationId,appRoleAssignmentRequired,passwordCredentials,keyCredentials";
/// The app role id of an assignment to an app that defines no roles.
const DEFAULT_ACCESS: &str = "00000000-0000-0000-0000-000000000000";

#[derive(clap::Args)]
pub struct ServiceprincipalGetArgs {
    /// The service principal or managed identity: object id, client id (as arm identity get and k8s serviceaccount get print it), a managed identity's ARM id, display name, or portal link
    serviceprincipal: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceprincipalDetail {
    /// The object id: what RBAC, ACLs and arm role list --principal take.
    id: String,
    /// The client id; entra app get takes it when the app is this tenant's.
    app_id: Option<String>,
    name: Option<String>,
    /// Application, ManagedIdentity or Legacy.
    #[serde(rename = "type")]
    kind: Option<String>,
    enabled: Option<bool>,
    /// A managed identity's ARM id, which arm identity get takes.
    arm: Option<String>,
    /// The tenant the app is registered in.
    home_tenant: Option<String>,
    /// Only users and apps assigned to it may sign in to it.
    assignment_required: Option<bool>,
    credentials: Vec<Credential>,
    /// Application permissions granted to it: each API and role.
    app_roles: Vec<AppRole>,
    /// Delegated permissions consented for it.
    delegated: Vec<Delegated>,
    /// Every group it is in, nested ones too.
    groups: Vec<Named>,
    owners: Vec<Owner>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AppRole {
    resource: String,
    role: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Delegated {
    resource: String,
    scopes: Vec<String>,
    /// admin (for every user) or user (one user's).
    consent: Option<String>,
}

fn serviceprincipal_get(
    ctx: &Ctx,
    args: ServiceprincipalGetArgs,
) -> Result<ServiceprincipalDetail> {
    let graph = Graph::new(ctx);
    let (_, sp) = find(
        &graph,
        &args.serviceprincipal,
        &[Kind::ServicePrincipal],
        SELECT,
    )?;
    let id = text(&sp["id"]).unwrap_or_default();
    let at = format!("{V1}/servicePrincipals/{id}");
    let api_name =
        |resource_id: &str, fallback: Option<String>| -> Result<(String, serde_json::Value)> {
            let held = resource(&graph, &format!("/{resource_id}"))?.unwrap_or_default();
            let name = text(&held["displayName"])
                .or(fallback)
                .unwrap_or_else(|| resource_id.to_owned());
            Ok((name, held))
        };
    let mut app_roles = Vec::new();
    let assigned = graph.pages(
        &format!("{at}/appRoleAssignments?$select=resourceId,resourceDisplayName,appRoleId"),
        usize::MAX,
    )?;
    for held in &assigned.rows {
        let Some(resource_id) = text(&held["resourceId"]) else {
            continue;
        };
        let role_id = text(&held["appRoleId"]).unwrap_or_default();
        let (name, api) = api_name(&resource_id, text(&held["resourceDisplayName"]))?;
        let role = if role_id == DEFAULT_ACCESS {
            "default access".to_owned()
        } else {
            api["appRoles"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|defined| text(&defined["id"]).as_deref() == Some(role_id.as_str()))
                .and_then(|defined| text(&defined["value"]))
                .unwrap_or(role_id)
        };
        app_roles.push(AppRole {
            resource: name,
            role,
        });
    }
    let mut delegated = Vec::new();
    let grants = graph.pages(&format!("{at}/oauth2PermissionGrants"), usize::MAX)?;
    for held in &grants.rows {
        let Some(resource_id) = text(&held["resourceId"]) else {
            continue;
        };
        let (name, _) = api_name(&resource_id, None)?;
        delegated.push(Delegated {
            resource: name,
            scopes: held["scope"]
                .as_str()
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            consent: match held["consentType"].as_str() {
                Some("AllPrincipals") => Some("admin".to_owned()),
                Some("Principal") => Some("user".to_owned()),
                _ => None,
            },
        });
    }
    let groups = graph.pages(
        &format!("{at}/transitiveMemberOf?$select=id,displayName"),
        usize::MAX,
    )?;
    let owners = owners(&graph, Kind::ServicePrincipal, &id)?;
    let enabled = sp["accountEnabled"].as_bool();
    if enabled == Some(false) {
        ctx.note(format!(
            "[next: agent-cli entra audit list --target {id} --since 30d]"
        ));
    }
    Ok(ServiceprincipalDetail {
        app_id: text(&sp["appId"]),
        name: text(&sp["displayName"]),
        kind: text(&sp["servicePrincipalType"]),
        enabled,
        // VERIFY(work): that a managed identity's alternativeNames holds its
        // ARM id after an `isExplicit=` entry, as a 2021 write-up shows.
        arm: arm_id(&sp),
        home_tenant: text(&sp["appOwnerOrganizationId"]),
        assignment_required: sp["appRoleAssignmentRequired"].as_bool(),
        credentials: credentials(&sp),
        app_roles,
        delegated,
        groups: named(&groups.rows, Kind::Group),
        owners,
        id,
    })
}

command! {
    pub SERVICEPRINCIPAL_GET = ["entra", "serviceprincipal", "get"], Read,
    "Show an enterprise app or identity in Entra: the app roles and scopes granted it",
    keywords: ["spn", "enterprise", "application", "msi", "consent", "api", "oauth2"],
    example: "entra serviceprincipal get 00000000-0000-4000-8000-00000000a1f1 --fields name,type,arm,app_roles,groups",
    run: serviceprincipal_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::entra::testing::{V1, entra, page, urls};

    const ARM: &str = "/subscriptions/00000000-0000-4000-8000-000000005b01/resourceGroups/rg-contoso-prod/providers/Microsoft.ManagedIdentity/userAssignedIdentities/id-contoso-airflow";
    const SP: &str = "00000000-0000-4000-8000-00000000a1f2";

    #[test]
    fn an_arm_id_finds_the_managed_identity_with_its_grants_named() {
        let (outcome, transport) = entra(
            &["entra", "serviceprincipal", "get", ARM],
            vec![
                page(vec![
                    json!({"id": SP, "appId": "00000000-0000-4000-8000-00000000a1f1",
                    "displayName": "id-contoso-airflow", "servicePrincipalType": "ManagedIdentity",
                    "accountEnabled": false, "alternativeNames": ["isExplicit=True", ARM],
                    "appOwnerOrganizationId": null, "passwordCredentials": [], "keyCredentials": []}),
                ]),
                page(vec![
                    json!({"resourceId": "r1", "resourceDisplayName": "Microsoft Graph",
                    "appRoleId": "df021288-bdef-4463-88db-98f22de89214"}),
                ]),
                Answer::json(
                    &json!({"id": "r1", "displayName": "Microsoft Graph", "appRoles": [
                    {"id": "df021288-bdef-4463-88db-98f22de89214", "value": "User.Read.All"}]}),
                ),
                page(vec![
                    json!({"resourceId": "r1", "consentType": "AllPrincipals", "scope": "User.Read openid"}),
                ]),
                Answer::json(&json!({"id": "r1", "displayName": "Microsoft Graph"})),
                page(vec![]),
                page(vec![]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["arm"], ARM);
        assert_eq!(got["type"], "ManagedIdentity");
        assert_eq!(
            got["app_roles"],
            json!([{"resource": "Microsoft Graph", "role": "User.Read.All"}])
        );
        assert_eq!(
            got["delegated"],
            json!([{"resource": "Microsoft Graph", "scopes": ["User.Read", "openid"], "consent": "admin"}])
        );
        assert!(
            outcome.stderr.contains(&format!(
                "[next: agent-cli entra audit list --target {SP} --since 30d]"
            )),
            "{}",
            outcome.stderr
        );
        assert!(
            urls(&transport)[0].starts_with(&format!(
                "{V1}/servicePrincipals?$filter=alternativeNames%2Fany%28x%3Ax%20eq%20%27%2Fsubscriptions%2F"
            )),
            "{:?}",
            urls(&transport)
        );
    }
}
