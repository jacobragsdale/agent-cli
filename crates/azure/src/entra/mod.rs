//! `entra`: Microsoft Entra ID through Microsoft Graph, read-only, with the
//! az login's token for `https://graph.microsoft.com/`. The tenant is the
//! login's; nothing is configured.
//!
//! A row's `id` is the object id. An app has three GUIDs: its object id,
//! its appId (the client id `arm identity get` and `k8s serviceaccount get`
//! print) and its service principal's object id (what RBAC, ACLs and `arm
//! role list --principal` take); rows name the other two as `app_id` and
//! `service_principal`. [`find`] reads every form a get takes: an object id,
//! an appId, a UPN or mail, `@me`, a managed identity's ARM id, a portal
//! link, a display name.
//!
//! [`Graph`] is the door: the token goes to `graph.microsoft.com` only,
//! checked on every URL, `@odata.nextLink`s included. It sends GETs and
//! `Request::query` POSTs, never a write, though the az token's scopes
//! (Directory.AccessAsUser.All, Group.ReadWrite.All …) would allow some.
//! Collections are split by `@odata.type` here rather than cast, so only
//! counts need `ConsistencyLevel: eventual` and its lagging index.

pub(crate) mod app;
pub(crate) mod audit;
mod find;
pub(crate) mod group;
pub(crate) mod member;
pub(crate) mod principal;
pub(crate) mod role;
pub(crate) mod serviceprincipal;
pub(crate) mod signin;
#[cfg(test)]
pub(crate) mod testing;
pub(crate) mod user;

use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Exit, Failure, Request, host_under};
use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{GRAPH, bearer, parse_stamp, stamp, text};
use crate::lake::encoded;

pub(crate) use find::{Kind, find, me};

pub(crate) const V1: &str = "https://graph.microsoft.com/v1.0";
/// Sign-ins of service principals and managed identities are in beta only.
pub(crate) const BETA: &str = "https://graph.microsoft.com/beta";
/// What a paged read leaves of the deadline for the command's own work.
const MARGIN: Duration = Duration::from_secs(5);
/// The most ids one `getByIds` takes.
const BY_IDS: usize = 1000;

/// The door to Microsoft Graph.
pub(crate) struct Graph<'c> {
    ctx: &'c Ctx,
}

/// A paged read: the rows, whether more were there, and whether the
/// deadline let it finish.
pub(crate) struct Paged {
    pub rows: Vec<Value>,
    pub more: bool,
    pub complete: bool,
}

impl<'c> Graph<'c> {
    pub fn new(ctx: &'c Ctx) -> Self {
        Self { ctx }
    }

    pub fn ctx(&self) -> &'c Ctx {
        self.ctx
    }

    fn send(&self, request: Request<'_>) -> Result<Value> {
        if !host_under(&request.url, "graph.microsoft.com") {
            bail!(
                "{} is not Microsoft Graph; the Graph token is not sent there",
                request.url
            );
        }
        let mint = bearer(self.ctx, GRAPH);
        self.ctx.read(request.auth(&mint)).map_err(explain)?.json()
    }

    pub fn get(&self, url: &str) -> Result<Value> {
        self.send(Request::get(url))
    }

    /// A `/$count`, which Graph answers only from its eventual index.
    pub fn count(&self, url: &str) -> Result<Option<u64>> {
        Ok(self
            .send(Request::get(url).header("ConsistencyLevel", "eventual"))?
            .as_u64())
    }

    /// A POST that only reads (`getByIds`, `getMemberObjects`).
    pub fn query(&self, url: &str, body: Value) -> Result<Value> {
        self.send(Request::query(url, body))
    }

    /// `url`'s `value`, following `@odata.nextLink` until `limit` rows and
    /// one more (to tell there are more), the last page, or the deadline.
    pub fn pages(&self, url: &str, limit: usize) -> Result<Paged> {
        let mut rows = Vec::new();
        let mut next = Some(url.to_owned());
        while let Some(url) = next.take() {
            if !rows.is_empty() && self.ctx.deadline() <= Instant::now() + MARGIN {
                return Ok(Paged {
                    rows,
                    more: true,
                    complete: false,
                });
            }
            let page = self.get(&url)?;
            rows.extend(page["value"].as_array().into_iter().flatten().cloned());
            if rows.len() > limit {
                rows.truncate(limit);
                return Ok(Paged {
                    rows,
                    more: true,
                    complete: true,
                });
            }
            next = text(&page["@odata.nextLink"]);
        }
        Ok(Paged {
            rows,
            more: false,
            complete: true,
        })
    }

    /// The objects with these ids, of these types (`user`, `group`,
    /// `servicePrincipal`, `application`), 1000 a call; a deleted one is
    /// simply not there.
    pub fn by_ids(&self, ids: &[String], types: &[&str]) -> Result<Vec<Value>> {
        let mut found = Vec::new();
        for chunk in ids.chunks(BY_IDS) {
            let answer = self.query(
                &format!("{V1}/directoryObjects/getByIds"),
                json!({"ids": chunk, "types": types}),
            )?;
            found.extend(answer["value"].as_array().into_iter().flatten().cloned());
        }
        Ok(found)
    }
}

/// Graph's refusals with the step that follows them.
fn explain(error: anyhow::Error) -> anyhow::Error {
    let mut failure = match error.downcast::<Failure>() {
        Ok(failure) => failure,
        Err(error) => return error,
    };
    let said = failure.message.clone();
    if said.contains("premium license") {
        failure.exit = Exit::Setup;
        failure.hint = Some("sign-in logs need Microsoft Entra ID P1 or P2 on the tenant; `agent-cli entra audit list` reads the directory's own log".to_owned());
    } else if said.contains("allowed roles") {
        failure.hint = Some("the logs need the login to hold Reports Reader, Security Reader or Global Reader; `agent-cli doctor entra` shows what it holds".to_owned());
    } else if failure.status == Some(403) {
        failure.hint = Some(
            "the login may not read this; `agent-cli doctor entra` shows what its token holds"
                .to_owned(),
        );
    } else if failure.status == Some(404) && failure.hint.is_none() {
        failure.hint =
            Some("`agent-cli entra principal list ID` says what an object id is".to_owned());
    }
    failure.into()
}

/// What `--status` keeps of a log: failures or successes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Status {
    Failed,
    Succeeded,
}

/// A value inside a `$filter`, quoted as OData quotes it.
pub(crate) fn odata(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "''"))
}

/// A query parameter's value, encoded.
pub(crate) fn param(raw: &str) -> String {
    encoded(raw, false)
}

/// One app registration or service principal credential, as the portal
/// lists it. Never its value: Graph returns a secret only when it is made,
/// and a certificate's bytes only when asked for by name; neither is.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Credential {
    /// `secret` or `certificate`.
    pub kind: String,
    /// What a sign-in's `key_id` names when it presents this one.
    pub key_id: Option<String>,
    pub name: Option<String>,
    pub starts: Option<String>,
    pub ends: Option<String>,
    pub expired: bool,
}

/// An object's `passwordCredentials` and `keyCredentials`, soonest end first.
pub(crate) fn credentials(object: &Value) -> Vec<Credential> {
    let now = agent_cli_core::now();
    let mut found: Vec<Credential> = [
        ("secret", "passwordCredentials"),
        ("certificate", "keyCredentials"),
    ]
    .iter()
    .flat_map(|(kind, key)| {
        object[*key]
            .as_array()
            .into_iter()
            .flatten()
            .map(move |held| Credential {
                kind: (*kind).to_owned(),
                key_id: text(&held["keyId"]),
                name: text(&held["displayName"]),
                starts: stamp(&held["startDateTime"]),
                ends: stamp(&held["endDateTime"]),
                expired: held["endDateTime"]
                    .as_str()
                    .and_then(parse_stamp)
                    .is_some_and(|ends| ends <= now),
            })
    })
    .collect();
    found.sort_by(|a, b| a.ends.cmp(&b.ends));
    found
}

/// One `{id, name}`: a group a principal is in, an owner.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Named {
    pub id: String,
    pub name: Option<String>,
}

/// Rows of `{id, displayName}` as `Named`, keeping only `kind`'s.
pub(crate) fn named(rows: &[Value], kind: Kind) -> Vec<Named> {
    rows.iter()
        .filter(|held| Kind::of(held) == Some(kind))
        .filter_map(|held| {
            Some(Named {
                id: text(&held["id"])?,
                name: text(&held["displayName"]),
            })
        })
        .collect()
}

/// An owner of a group, app or service principal.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Owner {
    pub id: String,
    pub name: Option<String>,
    pub upn: Option<String>,
    pub enabled: Option<bool>,
}

/// A managed identity's ARM id, from its service principal's
/// `alternativeNames` (an `isExplicit=…` entry, then the id).
pub(crate) fn arm_id(object: &Value) -> Option<String> {
    object["alternativeNames"]
        .as_array()?
        .iter()
        .filter_map(text)
        .find(|name| name.to_ascii_lowercase().starts_with("/subscriptions/"))
}

/// The service principal of an API an app asks for or was granted (Graph,
/// Azure Service Management …) with its roles and scopes, by `(appId='…')`
/// or `/ID`, cached: a handful of them serve every app in a tenant.
pub(crate) fn resource(graph: &Graph<'_>, by: &str) -> Result<Option<Value>> {
    let key = format!("entra/resource/{by}");
    if let Some(held) = graph.ctx().cache().get::<Value>(&key) {
        return Ok(Some(held));
    }
    let url = format!(
        "{V1}/servicePrincipals{by}?$select=id,appId,displayName,appRoles,oauth2PermissionScopes"
    );
    match graph.get(&url) {
        Ok(found) => {
            let refresh = crate::config::Azure::load(graph.ctx())
                .map(|azure| azure.refresh())
                .unwrap_or_default();
            graph.ctx().cache().put(&key, &found, refresh);
            Ok(Some(found))
        }
        Err(error) if agent_cli_core::status_of(&error) == Some(404) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn owners(graph: &Graph<'_>, kind: Kind, id: &str) -> Result<Vec<Owner>> {
    let found = graph.pages(
        &format!(
            "{V1}/{}/{id}/owners?$select=id,displayName,userPrincipalName,accountEnabled",
            kind.collection()
        ),
        usize::MAX,
    )?;
    Ok(found
        .rows
        .iter()
        .filter_map(|held| {
            Some(Owner {
                id: text(&held["id"])?,
                name: text(&held["displayName"]),
                upn: text(&held["userPrincipalName"]),
                enabled: held["accountEnabled"].as_bool(),
            })
        })
        .collect())
}
