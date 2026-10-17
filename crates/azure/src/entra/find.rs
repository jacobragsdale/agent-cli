//! Finding the one object an argument names: an object id, an app (client)
//! id, a UPN or mail, `@me`, a managed identity's ARM id, a portal link or a
//! display name, among the kinds a command takes.

use agent_cli_core::{Ctx, Failure, host_under};
use anyhow::Result;
use serde_json::Value;

use crate::client::{GRAPH, claims, guid, text};
use crate::entra::{Graph, V1, odata, param};

/// The kinds of object a get reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    User,
    Group,
    App,
    ServicePrincipal,
}

impl Kind {
    pub fn collection(self) -> &'static str {
        match self {
            Kind::User => "users",
            Kind::Group => "groups",
            Kind::App => "applications",
            Kind::ServicePrincipal => "servicePrincipals",
        }
    }

    /// Its name in `getByIds`' `types`.
    pub fn type_name(self) -> &'static str {
        match self {
            Kind::User => "user",
            Kind::Group => "group",
            Kind::App => "application",
            Kind::ServicePrincipal => "servicePrincipal",
        }
    }

    /// What rows call it.
    pub fn word(self) -> &'static str {
        match self {
            Kind::User => "user",
            Kind::Group => "group",
            Kind::App => "application",
            Kind::ServicePrincipal => "serviceprincipal",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Kind::User => "user",
            Kind::Group => "group",
            Kind::App => "app registration",
            Kind::ServicePrincipal => "service principal",
        }
    }

    fn get(self) -> &'static str {
        match self {
            Kind::User => "entra user get",
            Kind::Group => "entra group get",
            Kind::App => "entra app get",
            Kind::ServicePrincipal => "entra serviceprincipal get",
        }
    }

    /// What a multi-kind lookup selects: no property another kind lacks,
    /// which Graph refuses.
    fn brief(self) -> &'static str {
        match self {
            Kind::User => "id,displayName,userPrincipalName,mail,accountEnabled",
            Kind::Group => "id,displayName,groupTypes,securityEnabled,mailEnabled",
            Kind::App => "id,appId,displayName",
            Kind::ServicePrincipal => {
                "id,appId,displayName,servicePrincipalType,accountEnabled,alternativeNames"
            }
        }
    }

    /// The kind an object's `@odata.type` names.
    pub fn of(object: &Value) -> Option<Kind> {
        match object["@odata.type"].as_str()? {
            "#microsoft.graph.user" => Some(Kind::User),
            "#microsoft.graph.group" => Some(Kind::Group),
            "#microsoft.graph.application" => Some(Kind::App),
            "#microsoft.graph.servicePrincipal" => Some(Kind::ServicePrincipal),
            _ => None,
        }
    }
}

const EVERY: [Kind; 4] = [Kind::User, Kind::Group, Kind::ServicePrincipal, Kind::App];

/// The object id in the az login's Graph token: who `@me` is.
pub(crate) fn me(ctx: &Ctx) -> Result<String> {
    claims(ctx, GRAPH)?
        .and_then(|claims| text(&claims["oid"]))
        .ok_or_else(|| {
            Failure::setup("the az login's Graph token names no object id")
                .hint("pass an object id instead of @me")
                .into()
        })
}

/// The GUID a portal link names (`…/userId/ID`, `…/groupId/ID`,
/// `…/objectId/ID`, else an app's `…/appId/ID`), parsed, never fetched.
// VERIFY(work): the blades on pasted links; these are Maester's templates.
fn from_portal(raw: &str) -> Option<String> {
    if !(host_under(raw, "portal.azure.com") || host_under(raw, "entra.microsoft.com")) {
        return None;
    }
    let segments: Vec<&str> = raw.split(['/', '?', '&', '=']).collect();
    ["objectId", "userId", "groupId", "appId"]
        .iter()
        .find_map(|key| {
            let at = segments.iter().position(|held| held == key)?;
            segments
                .get(at + 1)
                .filter(|id| guid(id))
                .map(|id| id.to_ascii_lowercase())
        })
}

/// The one object `raw` names among `kinds`, with `select` when one kind is
/// asked for (each kind's brief properties otherwise). See the module doc
/// for the forms; a GUID of another kind is exit 2 naming its get, a name
/// matching several is exit 2 listing ten.
pub(crate) fn find(
    graph: &Graph<'_>,
    raw: &str,
    kinds: &[Kind],
    select: &str,
) -> Result<(Kind, Value)> {
    let raw = raw.trim();
    let one = kinds.len() == 1;
    let pick = |kind: Kind| if one { select } else { kind.brief() };
    let read = |kind: Kind, path: &str| {
        graph.get(&format!(
            "{V1}/{}{path}?$select={}",
            kind.collection(),
            pick(kind)
        ))
    };
    let missing = |error: &anyhow::Error| agent_cli_core::status_of(error) == Some(404);
    if raw == "@me" && kinds.contains(&Kind::User) {
        let id = me(graph.ctx())?;
        return Ok((Kind::User, read(Kind::User, &format!("/{id}"))?));
    }
    if raw.starts_with('/') && kinds.contains(&Kind::ServicePrincipal) {
        // VERIFY(work): that alternativeNames matches an ARM id in any case.
        let url = format!(
            "{V1}/servicePrincipals?$filter={}&$select={}",
            param(&format!("alternativeNames/any(x:x eq {})", odata(raw))),
            pick(Kind::ServicePrincipal)
        );
        let found = graph.get(&url)?;
        return match found["value"].as_array().and_then(|rows| rows.first()) {
            Some(held) => Ok((Kind::ServicePrincipal, held.clone())),
            None => Err(
                Failure::not_found(format!("no service principal holds {raw}"))
                    .hint(
                        "`agent-cli arm identity get ID` prints a managed identity's principal_id",
                    )
                    .into(),
            ),
        };
    }
    let id = from_portal(raw).unwrap_or_else(|| raw.to_ascii_lowercase());
    if guid(&id) {
        if one {
            let kind = kinds[0];
            match read(kind, &format!("/{id}")) {
                Ok(held) => return Ok((kind, held)),
                Err(error) if missing(&error) => {}
                Err(error) => return Err(error),
            }
            if matches!(kind, Kind::App | Kind::ServicePrincipal) {
                match read(kind, &format!("(appId='{id}')")) {
                    Ok(held) => return Ok((kind, held)),
                    Err(error) if missing(&error) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let types: Vec<&str> = if one {
            EVERY.iter().map(|kind| kind.type_name()).collect()
        } else {
            kinds.iter().map(|kind| kind.type_name()).collect()
        };
        if let Some(held) = graph
            .by_ids(std::slice::from_ref(&id), &types)?
            .into_iter()
            .next()
            && let Some(kind) = Kind::of(&held)
        {
            if kinds.contains(&kind) {
                return Ok((kind, held));
            }
            let sp = text(&held["servicePrincipalType"])
                .map(|kind| format!(" ({kind})"))
                .unwrap_or_default();
            return Err(Failure::usage(format!(
                "{id} is a {}{sp}, not a {}",
                kind.noun(),
                kinds[0].noun()
            ))
            .hint(format!("agent-cli {} {id}", kind.get()))
            .into());
        }
        if !one && kinds.contains(&Kind::ServicePrincipal) {
            match read(Kind::ServicePrincipal, &format!("(appId='{id}')")) {
                Ok(held) => return Ok((Kind::ServicePrincipal, held)),
                Err(error) if missing(&error) => {}
                Err(error) => return Err(error),
            }
        }
        return Err(Failure::not_found(format!(
            "no {} has id {id} in this tenant (deleted, or never here)",
            nouns(kinds)
        ))
        .hint(format!(
            "agent-cli entra audit list --target {id} --since 30d"
        ))
        .into());
    }
    if raw.contains('@') && kinds.contains(&Kind::User) {
        match read(Kind::User, &format!("/{}", param(raw))) {
            Ok(held) => return Ok((Kind::User, held)),
            Err(error) if missing(&error) => {}
            Err(error) => return Err(error),
        }
        let url = format!(
            "{V1}/users?$filter={}&$select={}",
            param(&format!("mail eq {}", odata(raw))),
            pick(Kind::User)
        );
        if let Some(held) = graph.get(&url)?["value"]
            .as_array()
            .and_then(|rows| rows.first())
        {
            return Ok((Kind::User, held.clone()));
        }
        return Err(
            Failure::not_found(format!("no user signs in as or has mail {raw}"))
                .hint("`agent-cli entra principal list ID` takes object ids")
                .into(),
        );
    }
    for filter in [
        format!("displayName eq {}", odata(raw)),
        format!("startswith(displayName,{})", odata(raw)),
    ] {
        for &kind in kinds {
            let url = format!(
                "{V1}/{}?$filter={}&$select={}&$top=11",
                kind.collection(),
                param(&filter),
                pick(kind)
            );
            let found = graph.get(&url)?;
            let rows = found["value"].as_array().cloned().unwrap_or_default();
            match rows.len() {
                0 => continue,
                1 => return Ok((kind, rows[0].clone())),
                many => {
                    let listed: Vec<String> = rows
                        .iter()
                        .take(10)
                        .map(|held| {
                            format!(
                                "{} ({})",
                                text(&held["displayName"]).unwrap_or_default(),
                                text(&held["id"]).unwrap_or_default()
                            )
                        })
                        .collect();
                    let more = if many > 10 { ", …" } else { "" };
                    return Err(Failure::usage(format!(
                        "several {}s are named {raw:?}: {}{more}",
                        kind.noun(),
                        listed.join(", ")
                    ))
                    .hint(format!("pass the id: agent-cli {} ID", kind.get()))
                    .into());
                }
            }
        }
    }
    Err(
        Failure::not_found(format!("no {} is named {raw:?}", nouns(kinds)))
            .hint("pass an object id; `agent-cli entra principal list ID` says what one is")
            .into(),
    )
}

fn nouns(kinds: &[Kind]) -> String {
    let words: Vec<&str> = kinds.iter().map(|kind| kind.noun()).collect();
    words.join(" or ")
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::entra::testing::{V1, entra, not_found, page, urls};

    const SP: &str = "00000000-0000-4000-8000-00000000a1f2";

    #[test]
    fn a_portal_link_names_its_object_and_other_hosts_are_not_read() {
        assert_eq!(
            super::from_portal(&format!("https://portal.azure.com/#view/Microsoft_AAD_IAM/ManagedAppMenuBlade/~/Overview/objectId/{SP}/appId/00000000-0000-4000-8000-00000000a1f1")).as_deref(),
            Some(SP)
        );
        assert_eq!(
            super::from_portal(&format!("https://contoso.example/objectId/{SP}")),
            None
        );
    }

    #[test]
    fn a_guid_of_another_kind_is_exit_2_naming_its_get_and_a_token_goes_only_to_graph() {
        let (outcome, transport) = entra(
            &["entra", "user", "get", SP],
            vec![
                not_found(),
                page(vec![
                    json!({"@odata.type": "#microsoft.graph.servicePrincipal", "id": SP,
                    "displayName": "id-contoso-airflow", "servicePrincipalType": "ManagedIdentity"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains(&format!(
                "{SP} is a service principal (ManagedIdentity), not a user"
            )) && outcome
                .stderr
                .contains(&format!("agent-cli entra serviceprincipal get {SP}")),
            "{}",
            outcome.stderr
        );
        let sent = transport.sent();
        assert_eq!(sent[1].url, format!("{V1}/directoryObjects/getByIds"));
        assert_eq!(
            sent[1].body.as_ref().unwrap()["types"],
            json!(["user", "group", "servicePrincipal", "application"])
        );
        assert!(sent.iter().all(|sent| {
            sent.authorization.as_deref() == Some("Bearer token@https://graph.microsoft.com/")
        }));
    }

    #[test]
    fn a_name_matching_several_lists_them_with_ids_and_a_refusal_names_doctor() {
        let (outcome, transport) = entra(
            &["entra", "group", "get", "sg-contoso"],
            vec![
                page(vec![]),
                page(vec![
                    json!({"id": "g1", "displayName": "sg-contoso-platform"}),
                    json!({"id": "g2", "displayName": "sg-contoso-oncall"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("sg-contoso-platform (g1), sg-contoso-oncall (g2)"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{V1}/groups?$filter=startswith%28displayName%2C%27sg-contoso%27%29&$select=id,displayName,description,groupTypes,mail,mailEnabled,securityEnabled,membershipRule,membershipRuleProcessingState,isAssignableToRole,onPremisesSyncEnabled,createdDateTime&$top=11"
            )
        );
        let (outcome, _) = entra(
            &["entra", "group", "get", "sg-contoso"],
            vec![Answer::status(
                403,
                r#"{"error":{"code":"Authorization_RequestDenied","message":"Insufficient privileges to complete the operation."}}"#,
            )],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("agent-cli doctor entra"),
            "{}",
            outcome.stderr
        );
    }
}
