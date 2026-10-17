//! `arm identity get`: one user-assigned identity by id, name or client id,
//! with its federated credentials (whose subject is named as the k8s
//! serviceaccount id when the issuer is an AKS cluster's) and its roles.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::arm::identity::{Federated, IDENTITY, federated};
use crate::arm::quote;
use crate::arm::role::{Assignment, assignments};
use crate::client::{guid, text};
use crate::config::Azure;
use crate::net::{Place, list, read};

/// Microsoft.ManagedIdentity's stable api-version with federated credentials.
const API: &str = "2023-01-31";

#[derive(clap::Args)]
pub struct IdentityGetArgs {
    /// The identity: its ARM id, portal link, name, or client id (as k8s serviceaccount get prints it)
    identity: String,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IdentityDetail {
    /// The ARM id.
    id: String,
    name: String,
    group: Option<String>,
    /// What a k8s serviceaccount's `azure.workload.identity/client-id` names.
    client_id: Option<String>,
    /// What roles are granted to: `arm role list --principal` takes it.
    principal_id: Option<String>,
    tenant_id: Option<String>,
    federated_credentials: Vec<Federated>,
    /// Its role assignments, each with the scope it is granted at.
    roles: Vec<Assignment>,
}

fn identity_get(ctx: &Ctx, args: IdentityGetArgs) -> Result<IdentityDetail> {
    let azure = Azure::load(ctx)?;
    let raw = args.identity.trim();
    let id = if guid(raw) {
        let tail = format!(
            "\n| where tostring(properties.clientId) =~ {}\n| project id\n| order by id asc",
            quote(raw)
        );
        let found = args.place.rows(ctx, &azure, &[IDENTITY], &tail, 1)?;
        found
            .first()
            .and_then(|row| text(&row["id"]))
            .ok_or_else(|| {
                Failure::not_found(format!("no user-assigned identity has client id {raw}"))
                    .hint("the ones in reach: agent-cli arm identity list")
            })?
    } else {
        args.place
            .find(
                ctx,
                &azure,
                raw,
                &[IDENTITY],
                "arm identity list",
                "arm identity get",
            )?
            .id
    };
    let identity = read(ctx, &id, API, "arm identity list")?;
    let credentials = read(
        ctx,
        &format!("{id}/federatedIdentityCredentials"),
        API,
        "arm identity list",
    )?;
    let federated_credentials = federated(ctx, list(&credentials["value"]))?;
    let props = &identity["properties"];
    let principal_id = text(&props["principalId"]);
    let roles = match &principal_id {
        Some(principal) => assignments(ctx, &azure, &[principal], None, usize::MAX)?,
        None => Vec::new(),
    };
    Ok(IdentityDetail {
        name: text(&identity["name"]).unwrap_or_default(),
        group: crate::arm::resource::target(&id).and_then(|target| target.group),
        id,
        client_id: text(&props["clientId"]),
        principal_id,
        tenant_id: text(&props["tenantId"]),
        federated_credentials,
        roles,
    })
}

command! {
    pub IDENTITY_GET = ["arm", "identity", "get"], Read,
    "Show a managed identity: client id, federated credentials (k8s subject), roles",
    keywords: ["workload", "federated", "credential", "oidc", "issuer", "subject", "msi", "uami", "client"],
    example: "arm identity get 00000000-0000-4000-8000-00000000b0f1 --fields federated_credentials,roles",
    run: identity_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{azure, inventory};

    const ID: &str = "/subscriptions/00000000-0000-4000-8000-000000000001/resourceGroups/rg-contoso/providers/Microsoft.ManagedIdentity/userAssignedIdentities/id-contoso";
    const ISSUER: &str = "https://eastus.oic.prod-aks.azure.com/t/c/";

    #[test]
    fn a_client_id_finds_the_identity_and_its_credentials_and_roles_come_with_it() {
        let (outcome, transport) = azure(
            &[crate::ARM],
            &[
                "arm",
                "identity",
                "get",
                "00000000-0000-4000-8000-0000000000c1",
            ],
            vec![
                inventory(vec![json!({ "id": ID })]),
                Answer::json(&json!({"id": ID, "name": "id-contoso", "properties": {
                    "clientId": "00000000-0000-4000-8000-0000000000c1",
                    "principalId": "00000000-0000-4000-8000-0000000000c2",
                    "tenantId": "00000000-0000-4000-8000-0000000000c3"}})),
                Answer::json(
                    &json!({"value": [{"name": "aks-web-notifier", "properties": {
                    "issuer": ISSUER, "subject": "system:serviceaccount:web:notifier",
                    "audiences": ["api://AzureADTokenExchange"]}}]}),
                ),
                inventory(vec![json!({"name": "aks-contoso", "issuer": ISSUER})]),
                inventory(vec![
                    json!({"id": "/subscriptions/00000000-0000-4000-8000-000000000001/providers/Microsoft.Authorization/roleAssignments/r1",
                    "principalId": "00000000-0000-4000-8000-0000000000c2", "principalType": "ServicePrincipal",
                    "role": "Reader", "scope": "/subscriptions/00000000-0000-4000-8000-000000000001"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["group"], "rg-contoso");
        assert_eq!(
            got["federated_credentials"][0]["serviceaccount"],
            "aks-contoso/web/notifier"
        );
        assert_eq!(got["roles"][0]["role"], "Reader");
        let sent = transport.sent();
        assert!(
            sent[0].body.as_ref().unwrap()["query"]
                .as_str()
                .unwrap()
                .contains("| where tostring(properties.clientId) =~ '00000000-0000-4000-8000-0000000000c1'")
        );
        assert_eq!(
            sent[2].url,
            format!(
                "https://management.azure.com{ID}/federatedIdentityCredentials?api-version=2023-01-31"
            )
        );
        assert!(
            sent[4].body.as_ref().unwrap()["query"]
                .as_str()
                .unwrap()
                .contains("principalId =~ '00000000-0000-4000-8000-0000000000c2'")
        );

        let (outcome, _) = azure(
            &[crate::ARM],
            &[
                "arm",
                "identity",
                "get",
                "00000000-0000-4000-8000-0000000000c9",
            ],
            vec![inventory(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
