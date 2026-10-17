//! `entra principal list`: what each of up to a thousand GUIDs is, from one
//! `POST /directoryObjects/getByIds`; a GUID that is no object id is tried
//! as an app (client) id, and a UPN is read as a user.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{guid, limited, text};
use crate::entra::{Graph, Kind, V1, arm_id, me, odata, param};

/// The most values Graph's `in` takes in one filter.
// VERIFY(work): 15 is the documented cap for `in` on most properties.
const IN: usize = 15;

#[derive(clap::Args)]
pub struct PrincipalListArgs {
    /// Object ids (arm role list's principal_id, a lake ACL's entries), app (client) ids, UPNs, or @me
    #[arg(required = true)]
    ids: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrincipalRow {
    /// The object id, which the get of its type takes.
    id: String,
    /// user, group, serviceprincipal or application.
    #[serde(rename = "type")]
    kind: String,
    name: Option<String>,
    upn: Option<String>,
    /// An app's or service principal's client id.
    app_id: Option<String>,
    /// A service principal's: Application, ManagedIdentity or Legacy.
    sp_type: Option<String>,
    /// A managed identity's ARM id, which arm identity get takes.
    arm: Option<String>,
    enabled: Option<bool>,
}

fn row(object: &Value) -> Option<PrincipalRow> {
    Some(PrincipalRow {
        id: text(&object["id"])?,
        kind: Kind::of(object)?.word().to_owned(),
        name: text(&object["displayName"]),
        upn: text(&object["userPrincipalName"]),
        app_id: text(&object["appId"]),
        sp_type: text(&object["servicePrincipalType"]),
        arm: arm_id(object),
        enabled: object["accountEnabled"].as_bool(),
    })
}

fn principal_list(ctx: &Ctx, args: PrincipalListArgs) -> Result<Vec<PrincipalRow>> {
    let graph = Graph::new(ctx);
    let mut asked: Vec<String> = Vec::new();
    for raw in &args.ids {
        let raw = raw.trim();
        let held = match raw {
            "@me" => me(ctx)?,
            _ if guid(raw) => raw.to_ascii_lowercase(),
            _ if raw.contains('@') => raw.to_owned(),
            _ => {
                return Err(Failure::usage(format!(
                    "{raw:?} is no object id, app id or UPN"
                ))
                .hint("a name goes to the get of its kind: agent-cli entra serviceprincipal get NAME")
                .into());
            }
        };
        if !asked.contains(&held) {
            asked.push(held);
        }
    }
    let guids: Vec<String> = asked.iter().filter(|id| guid(id)).cloned().collect();
    let mut found: Vec<(String, Value)> = graph
        .by_ids(
            &guids,
            &["user", "group", "servicePrincipal", "application"],
        )?
        .into_iter()
        .filter_map(|held| Some((text(&held["id"])?.to_ascii_lowercase(), held)))
        .collect();
    let unknown: Vec<&String> = guids
        .iter()
        .filter(|id| !found.iter().any(|(held, _)| held == *id))
        .collect();
    for chunk in unknown.chunks(IN) {
        let quoted: Vec<String> = chunk.iter().map(|id| odata(id)).collect();
        let url = format!(
            "{V1}/servicePrincipals?$filter={}&$select=id,appId,displayName,servicePrincipalType,accountEnabled,alternativeNames",
            param(&format!("appId in ({})", quoted.join(",")))
        );
        for held in graph.get(&url)?["value"].as_array().into_iter().flatten() {
            if let Some(app) = text(&held["appId"]) {
                let mut held = held.clone();
                held["@odata.type"] = "#microsoft.graph.servicePrincipal".into();
                found.push((app.to_ascii_lowercase(), held));
            }
        }
    }
    for upn in asked.iter().filter(|id| !guid(id)) {
        let url = format!(
            "{V1}/users/{}?$select=id,displayName,userPrincipalName,accountEnabled",
            param(upn)
        );
        match graph.get(&url) {
            Ok(mut held) => {
                held["@odata.type"] = "#microsoft.graph.user".into();
                found.push((upn.clone(), held));
            }
            Err(error) if agent_cli_core::status_of(&error) == Some(404) => {}
            Err(error) => return Err(error),
        }
    }
    let mut rows = Vec::new();
    let mut missing = Vec::new();
    for id in &asked {
        match found.iter().find(|(held, _)| held.eq_ignore_ascii_case(id)) {
            Some((_, object)) => rows.extend(row(object)),
            None => missing.push(id.clone()),
        }
    }
    if rows.is_empty() {
        return Err(Failure::not_found(format!(
            "nothing in this tenant has id {} (deleted, or never here)",
            missing.join(", ")
        ))
        .hint(format!(
            "agent-cli entra audit list --target {} --since 30d",
            missing[0]
        ))
        .into());
    }
    if !missing.is_empty() {
        ctx.note(format!(
            "[not in this tenant, deleted or never here: {}]",
            missing.join(", ")
        ));
    }
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub PRINCIPAL_LIST = ["entra", "principal", "list"], Read,
    "Name the users, groups, apps or identities behind GUIDs, in one call",
    keywords: ["guid", "resolve", "lookup", "orphaned", "object", "name"],
    example: "entra principal list 00000000-0000-4000-8000-00000000a2f2 00000000-0000-4000-8000-00000000f001 --fields id,type,name",
    run: principal_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::entra::testing::{V1, entra, not_found, page, urls};

    const SP: &str = "00000000-0000-4000-8000-00000000a2f2";
    const APP: &str = "00000000-0000-4000-8000-00000000a1f1";
    const GONE: &str = "00000000-0000-4000-8000-0000000000ff";

    #[test]
    fn ids_are_named_in_one_call_app_ids_and_upns_after_and_the_missing_noted() {
        let (outcome, transport) = entra(
            &[
                "entra",
                "principal",
                "list",
                SP,
                APP,
                GONE,
                "sam@contoso.example",
                "nobody@contoso.example",
            ],
            vec![
                page(vec![
                    json!({"@odata.type": "#microsoft.graph.servicePrincipal", "id": SP,
                    "displayName": "contoso-finance-close", "appId": "00000000-0000-4000-8000-00000000a2f1",
                    "servicePrincipalType": "Application", "accountEnabled": true}),
                ]),
                page(vec![
                    json!({"id": "00000000-0000-4000-8000-00000000a1f2", "appId": APP,
                    "displayName": "id-contoso-airflow", "servicePrincipalType": "ManagedIdentity",
                    "alternativeNames": ["isExplicit=True", "/subscriptions/s/resourceGroups/rg/providers/Microsoft.ManagedIdentity/userAssignedIdentities/id-contoso-airflow"]}),
                ]),
                Answer::json(&json!({"id": "00000000-0000-4000-8000-00000000e102",
                    "displayName": "Sam Lee", "userPrincipalName": "sam@contoso.example", "accountEnabled": true})),
                not_found(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got,
            json!([
                {"id": SP, "type": "serviceprincipal", "name": "contoso-finance-close",
                 "app_id": "00000000-0000-4000-8000-00000000a2f1", "sp_type": "Application", "enabled": true},
                {"id": "00000000-0000-4000-8000-00000000a1f2", "type": "serviceprincipal", "name": "id-contoso-airflow",
                 "app_id": APP, "sp_type": "ManagedIdentity",
                 "arm": "/subscriptions/s/resourceGroups/rg/providers/Microsoft.ManagedIdentity/userAssignedIdentities/id-contoso-airflow"},
                {"id": "00000000-0000-4000-8000-00000000e102", "type": "user", "name": "Sam Lee",
                 "upn": "sam@contoso.example", "enabled": true},
            ])
        );
        assert!(
            outcome.stderr.contains(&format!(
                "[not in this tenant, deleted or never here: {GONE}, nobody@contoso.example]"
            )),
            "{}",
            outcome.stderr
        );
        let sent = urls(&transport);
        assert_eq!(sent[0], format!("{V1}/directoryObjects/getByIds"));
        assert_eq!(
            sent[1],
            format!(
                "{V1}/servicePrincipals?$filter=appId%20in%20%28%27{APP}%27%2C%27{GONE}%27%29&$select=id,appId,displayName,servicePrincipalType,accountEnabled,alternativeNames"
            )
        );
        assert_eq!(
            sent[2],
            format!(
                "{V1}/users/sam%40contoso.example?$select=id,displayName,userPrincipalName,accountEnabled"
            )
        );
    }

    #[test]
    fn nothing_found_is_exit_4_naming_the_audit_log_and_a_name_is_exit_2() {
        let (outcome, _) = entra(
            &["entra", "principal", "list", GONE],
            vec![page(vec![]), page(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains(&format!(
                "agent-cli entra audit list --target {GONE} --since 30d"
            )),
            "{}",
            outcome.stderr
        );
        let (outcome, _) = entra(
            &["entra", "principal", "list", "sg-contoso-platform"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
