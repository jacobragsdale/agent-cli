//! `arm identity list`: user-assigned managed identities from Resource Graph,
//! with the client id a workload identity names and the principal id roles
//! are granted to.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::arm::identity::IDENTITY;
use crate::client::text;
use crate::config::Azure;
use crate::net::Place;

#[derive(clap::Args)]
pub struct IdentityListArgs {
    #[command(flatten)]
    place: Place,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IdentityRow {
    /// The ARM id, which `arm identity get` takes.
    id: String,
    name: String,
    group: Option<String>,
    /// What a k8s serviceaccount's `azure.workload.identity/client-id` names.
    client_id: Option<String>,
    /// What `arm role list --principal` takes.
    principal_id: Option<String>,
}

fn identity_list(ctx: &Ctx, args: IdentityListArgs) -> Result<Vec<IdentityRow>> {
    let azure = Azure::load(ctx)?;
    let project = "id, name, resourceGroup, clientId = tostring(properties.clientId), \
                   principalId = tostring(properties.principalId)";
    Ok(args
        .place
        .list(ctx, &azure, IDENTITY, project, args.limit)?
        .iter()
        .map(|row| IdentityRow {
            id: text(&row["id"]).unwrap_or_default(),
            name: text(&row["name"]).unwrap_or_default(),
            group: text(&row["resourceGroup"]),
            client_id: text(&row["clientId"]),
            principal_id: text(&row["principalId"]),
        })
        .collect())
}

command! {
    pub IDENTITY_LIST = ["arm", "identity", "list"], Read,
    "List user-assigned managed identities with their client and principal ids",
    keywords: ["managed", "msi", "workload", "uami", "client"],
    example: "arm identity list --group rg-contoso-prod --fields id,client_id",
    run: identity_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{azure, inventory};

    #[test]
    fn identities_come_from_one_resource_graph_query() {
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "identity", "list", "--group", "rg-contoso"],
            vec![inventory(vec![json!({
                "id": "/subscriptions/00000000-0000-4000-8000-000000000001/resourceGroups/rg-contoso/providers/Microsoft.ManagedIdentity/userAssignedIdentities/id-contoso",
                "name": "id-contoso", "resourceGroup": "rg-contoso",
                "clientId": "00000000-0000-4000-8000-0000000000c1", "principalId": ""})])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[0]["client_id"],
            "00000000-0000-4000-8000-0000000000c1"
        );
        assert!(outcome.json()[0].get("principal_id").is_none());
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert_eq!(
            query,
            "resources\n| where type =~ 'Microsoft.ManagedIdentity/userAssignedIdentities'\n| where resourceGroup =~ 'rg-contoso'\n| project id, name, resourceGroup, clientId = tostring(properties.clientId), principalId = tostring(properties.principalId)\n| order by name asc, id asc"
        );
    }
}
