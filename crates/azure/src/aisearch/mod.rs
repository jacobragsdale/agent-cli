//! `aisearch`: Azure AI Search, over its REST data plane and ARM.
//!
//! Services come from the shared Resource Graph inventory, so every service
//! the login reaches works from the first command. Each service is called the
//! way its own settings allow, never as configured: a token for
//! `https://search.azure.com` when it takes roles (`disableLocalAuth`, or
//! `authOptions.aadOrApiKey`), else an admin key fetched from ARM for this run
//! only. A request carries one or the other, never both, and goes to no host
//! but one under `.search.windows.net`. A token refused by a service that
//! also takes keys is tried once more with a key.

pub(crate) mod document;
pub(crate) mod index;
pub(crate) mod indexer;
pub(crate) mod refs;
pub(crate) mod service;
pub(crate) mod shape;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_cli_core::{Ctx, Effect, Failure, Method, Request, Response, Secret, host_under};
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{ARM, Kind, SEARCH, bearer, narrow, parallel, refused_with, text};
use crate::config::Azure;
use crate::graph::{SearchService, inventory};

/// The data plane's GA version, where every read here is GA. Previews changed
/// list paging twice in 2026, so none is used.
pub(crate) const API_VERSION: &str = "2026-04-01";
/// ARM's, for `listAdminKeys` and a service's properties.
const ARM_API: &str = "2025-05-01";

pub(crate) const SERVICES: Kind = Kind {
    noun: "search service",
    key: "search_services",
    flag: "--service",
    domain: "aisearch",
};

/// What a call needs of the login, named when a token is refused.
#[derive(Clone, Copy)]
pub(crate) enum Role {
    /// Searching and reading documents.
    Query,
    /// Definitions, statistics and indexer status.
    Definitions,
    /// Creating, changing and running indexes and indexers.
    Manage,
    /// Uploading and deleting documents.
    Documents,
}

impl Role {
    fn names(self) -> &'static str {
        match self {
            Self::Query => "Search Index Data Reader",
            Self::Definitions => "Reader or Search Service Contributor",
            Self::Manage => "Search Service Contributor",
            Self::Documents => "Search Index Data Contributor",
        }
    }
}

/// Whether the service takes a token: roles only, or roles and keys. A new
/// service takes keys only.
pub(crate) fn takes_token(service: &SearchService) -> bool {
    service.local_auth_disabled == Some(true)
        || service.auth_options.eq_ignore_ascii_case("aadOrApiKey")
}

/// What rows print as `auth`: what the CLI will send.
pub(crate) fn auth_mode(service: &SearchService) -> &'static str {
    if takes_token(service) { "token" } else { "key" }
}

/// A service's ARM address, with `action` (`/listAdminKeys`) after it.
fn arm_url(service: &SearchService, action: &str) -> String {
    format!(
        "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Search/searchServices/{}{action}?api-version={ARM_API}",
        service.subscription, service.resource_group, service.name
    )
}

/// The services in reach, within `[azure] search_services` and then `only`
/// (names, or their endpoints' hosts). A row that lacks the endpoint or the
/// auth settings is completed from ARM, once per `refresh`.
pub(crate) fn reach(ctx: &Ctx, azure: &Azure, only: &[String]) -> Result<Vec<SearchService>> {
    let found = inventory(ctx, azure)?.search_services;
    let only: Vec<String> = only
        .iter()
        .map(|raw| refs::service_name(&found, raw))
        .collect();
    let services = narrow(&found, &azure.search_services, &only, SERVICES, |s| &s.name)?;
    parallel(&services, azure.parallel(), |service| {
        completed(ctx, azure, service)
    })
    .into_iter()
    .collect()
}

fn completed(ctx: &Ctx, azure: &Azure, service: &SearchService) -> Result<SearchService> {
    if service.complete() {
        return Ok(service.clone());
    }
    let key = format!(
        "azure:aisearch:{}/{}/{}",
        service.subscription, service.resource_group, service.name
    );
    if let Some(held) = ctx.cache().get::<SearchService>(&key) {
        return Ok(held);
    }
    let answer = ctx
        .read(Request::get(arm_url(service, "")).auth(&bearer(ctx, ARM)))
        .with_context(|| format!("reading {}'s settings from ARM", service.name))?
        .json()?;
    let mut filled = service.clone();
    filled.fill(&answer["properties"]);
    ctx.cache().put(&key, &filled, azure.refresh());
    Ok(filled)
}

/// The one service `name` names, or the only one in reach.
pub(crate) fn one(ctx: &Ctx, azure: &Azure, name: Option<&str>) -> Result<SearchService> {
    let only: Vec<String> = name.map(str::to_owned).into_iter().collect();
    let mut services = reach(ctx, azure, &only)?;
    match services.len() {
        0 => Err(none_in_reach()),
        1 => Ok(services.remove(0)),
        _ => Err(Failure::usage(format!(
            "there is more than one search service ({}); name one with --service",
            names(&services)
        ))
        .hint("agent-cli aisearch service list --fields id,endpoint")
        .into()),
    }
}

fn none_in_reach() -> anyhow::Error {
    Failure::setup("the login reaches no AI Search services")
        .hint("az login, or check the subscriptions with `agent-cli doctor aisearch`")
        .into()
}

pub(crate) fn names(services: &[SearchService]) -> String {
    services
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The service holding the index or indexer `name` (`what` is `indexes` or
/// `indexers`): `service`, or the only one in reach, or the one whose list
/// names it. Two holders is exit 2, never a guess.
// ponytail: a bare alias is not looked for; with --service or a SERVICE/
// prefix it works, as the service resolves it.
pub(crate) fn holder<'a>(
    ctx: &'a Ctx,
    azure: &Azure,
    what: &str,
    name: &str,
    service: Option<&str>,
) -> Result<Search<'a>> {
    if let Some(service) = service {
        return Search::new(ctx, one(ctx, azure, Some(service))?);
    }
    let services = reach(ctx, azure, &[])?;
    match services.as_slice() {
        [] => return Err(none_in_reach()),
        [only] => return Search::new(ctx, only.clone()),
        _ => {}
    }
    let sessions = services
        .into_iter()
        .map(|service| Search::new(ctx, service))
        .collect::<Result<Vec<_>>>()?;
    let listed = parallel(&sessions, azure.parallel(), |search| {
        search.get(&format!("/{what}?$select=name"), Role::Definitions)
    });
    let mut holding = Vec::new();
    let mut failed = Vec::new();
    for (search, listed) in sessions.into_iter().zip(listed) {
        match listed {
            Ok(answer)
                if list(&answer["value"])
                    .iter()
                    .any(|item| item["name"] == name) =>
            {
                holding.push(search);
            }
            Ok(_) => {}
            Err(error) => failed.push(error),
        }
    }
    // A service that did not answer can be ruled neither in nor out.
    if holding.is_empty()
        && let Some(error) = failed.into_iter().next()
    {
        return Err(error);
    }
    let noun = what.trim_end_matches("es").trim_end_matches('s');
    match holding.len() {
        1 => Ok(holding.remove(0)),
        0 => Err(
            Failure::not_found(format!("no {noun} {name} on any search service"))
                .hint(format!("agent-cli aisearch {noun} list --fields id"))
                .into(),
        ),
        _ => Err(Failure::usage(format!(
            "{name} is on more than one search service ({}); pass SERVICE/{name} or --service",
            holding
                .iter()
                .map(|s| s.service.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .hint(format!("agent-cli aisearch {noun} list --fields id"))
        .into()),
    }
}

/// One path segment: a name or a document key, percent-encoded.
pub(crate) fn segment(raw: &str) -> String {
    let mut encoded = String::new();
    agent_cli_core::percent_encode(raw, &mut encoded);
    encoded
}

/// The items of a JSON array, or none.
pub(crate) fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

// ---------- the door ----------

/// One service for one command. Every data-plane call goes through
/// [`Search::send`], which checks the host and attaches the token or the key.
pub(crate) struct Search<'a> {
    ctx: &'a Ctx,
    pub service: SearchService,
    base: String,
    /// The admin key, fetched at most once a run and never cached on disk.
    key: Mutex<Option<Secret>>,
    /// Set once the service refused the token and took the key: later calls
    /// go straight to the key.
    keyed: AtomicBool,
}

impl<'a> Search<'a> {
    pub(crate) fn new(ctx: &'a Ctx, service: SearchService) -> Result<Self> {
        let base = match service.endpoint.trim().trim_end_matches('/') {
            "" => format!("https://{}.search.windows.net", service.name),
            held => held.to_owned(),
        };
        if !host_under(&format!("{base}/"), ".search.windows.net") {
            bail!(
                "{}: {base} is not an AI Search address; no token or key is sent there",
                service.name
            );
        }
        Ok(Self {
            ctx,
            service,
            base,
            key: Mutex::new(None),
            keyed: AtomicBool::new(false),
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.service.name
    }

    pub(crate) fn url(&self, path: &str) -> String {
        let separator = if path.contains('?') { '&' } else { '?' };
        format!("{}{path}{separator}api-version={API_VERSION}", self.base)
    }

    pub(crate) fn get(&self, path: &str, role: Role) -> Result<Value> {
        self.send(None, Method::Get, path, None, &[], role)?.json()
    }

    /// A POST that only reads, such as a search.
    pub(crate) fn query(&self, path: &str, body: Value, role: Role) -> Result<Response> {
        self.send(None, Method::Query, path, Some(body), &[], role)
    }

    /// A change, through `ctx.write`.
    pub(crate) fn change(
        &self,
        effect: Effect,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, String)],
        role: Role,
    ) -> Result<Response> {
        self.send(Some(effect), method, path, body, headers, role)
    }

    fn send(
        &self,
        effect: Option<Effect>,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, String)],
        role: Role,
    ) -> Result<Response> {
        let url = self.url(path);
        if !host_under(&url, ".search.windows.net") {
            bail!("{url} is not an AI Search address; no token or key is sent there");
        }
        let mint = bearer(self.ctx, SEARCH);
        let attempt = |key: Option<&Secret>| {
            let mut request = Request::new(method, url.clone()).quota_429();
            for (name, value) in headers {
                request = request.header(*name, value.clone());
            }
            if let Some(body) = &body {
                request = request.json(body.clone());
            }
            request = match key {
                Some(key) => request.header("api-key", key.expose()),
                None => request.auth(&mint),
            };
            match effect {
                None => self.ctx.read(request),
                Some(effect) => self.ctx.write(effect, request),
            }
        };
        if !takes_token(&self.service) || self.keyed.load(Ordering::Relaxed) {
            let key = self.key()?;
            return attempt(Some(&key)).map_err(|error| self.explain(error, role, true));
        }
        match attempt(None) {
            Err(error)
                if matches!(refused_with(&error), Some(401 | 403))
                    && self
                        .service
                        .auth_options
                        .eq_ignore_ascii_case("aadOrApiKey") =>
            {
                let key = match self.key() {
                    Ok(key) => key,
                    Err(fetch) => {
                        let words =
                            format!("; an admin key could not be fetched either ({fetch:#})");
                        return Err(also(self.explain(error, role, false), &words));
                    }
                };
                match attempt(Some(&key)) {
                    Ok(answer) => {
                        self.keyed.store(true, Ordering::Relaxed);
                        Ok(answer)
                    }
                    Err(second) if matches!(refused_with(&second), Some(401 | 403)) => Err(also(
                        self.explain(error, role, false),
                        "; its admin key was refused too",
                    )),
                    Err(second) => Err(self.explain(second, role, true)),
                }
            }
            other => other.map_err(|error| self.explain(error, role, false)),
        }
    }

    /// The service's primary admin key, from ARM's `listAdminKeys`: a POST
    /// that only reads. Held in a `Secret` for this run, never in the cache.
    fn key(&self) -> Result<Secret> {
        let mut held = self
            .key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(key) = held.as_ref() {
            return Ok(key.clone());
        }
        let url = arm_url(&self.service, "/listAdminKeys");
        if !host_under(&url, "management.azure.com") {
            bail!("{url} is not ARM; the ARM token is not sent there");
        }
        let answer = self
            .ctx
            .read(Request::new(Method::Query, url).auth(&bearer(self.ctx, ARM)))
            .map_err(|error| match refused_with(&error) {
                Some(401 | 403) => Failure::setup(format!(
                    "{}: fetching its admin key was refused ({error:#})",
                    self.name()
                ))
                .hint(format!(
                    "it takes Contributor or Search Service Contributor on {}, or a service that accepts tokens (authOptions aadOrApiKey); then `agent-cli doctor aisearch`",
                    self.name()
                ))
                .into(),
                _ => error,
            })?
            .json()?;
        let key = Secret::new(text(&answer["primaryKey"]).with_context(|| {
            format!(
                "ARM answered listAdminKeys for {} without a primaryKey",
                self.name()
            )
        })?);
        *held = Some(key.clone());
        Ok(key)
    }

    /// A refusal in fewer, truer words: 401 and 403 are a missing role (exit
    /// 3), a 429 is the tier's quota rather than a rate, and a 402 is the
    /// free semantic quota.
    fn explain(&self, error: anyhow::Error, role: Role, by_key: bool) -> anyhow::Error {
        let Some(status) = refused_with(&error) else {
            return error;
        };
        let failure = match error.downcast::<Failure>() {
            Ok(failure) => failure,
            Err(error) => return error,
        };
        let name = self.name();
        match status {
            401 | 403 => {
                let message = if by_key {
                    format!("{}; {name} refused its own admin key", failure.message)
                } else {
                    format!(
                        "{}; the az login needs {} on {name}",
                        failure.message,
                        role.names()
                    )
                };
                let mut refused = Failure::setup(message).hint(format!(
                    "ask for {} on {name} (it applies in 5 to 10 minutes; Owner and Contributor give no data access by token), then `agent-cli doctor aisearch`",
                    role.names()
                ));
                refused.status = Some(status);
                refused.into()
            }
            429 => failure
                .hint(format!(
                    "an AI Search 429 is the tier's quota, not a rate (too many objects, or the storage is full): see `agent-cli aisearch service get {name}`"
                ))
                .into(),
            402 => failure
                .hint("the free semantic ranker quota is spent for the month: drop --semantic, or move the service to the standard semantic plan")
                .into(),
            _ => failure.into(),
        }
    }
}

/// `error` with `words` after its message, when it is a `Failure`.
fn also(error: anyhow::Error, words: &str) -> anyhow::Error {
    match error.downcast::<Failure>() {
        Ok(mut failure) => {
            failure.message.push_str(words);
            failure.into()
        }
        Err(error) => error,
    }
}

// ---------- several ids ----------

/// What a verb taking `ID…` prints: the object for one id, an array in the
/// order given for several.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Each<T> {
    One(T),
    Several(Vec<T>),
}

/// `one` over each of `ids`. One id answers or fails as `one` does; with
/// several, every id has its turn, and a failed one fails the command with
/// the others as data.
pub(crate) fn each<T: Serialize>(
    ids: &[String],
    mut one: impl FnMut(&str) -> Result<T>,
) -> Result<Each<T>> {
    if let [id] = ids {
        return one(id).map(Each::One);
    }
    let (mut rows, mut failed) = (Vec::new(), Vec::new());
    for id in ids {
        match one(id) {
            Ok(row) => rows.push(row),
            Err(error) => failed.push((id, error)),
        }
    }
    let Some((_, first)) = failed.first() else {
        return Ok(Each::Several(rows));
    };
    let Some(first) = first.chain().find_map(|c| c.downcast_ref::<Failure>()) else {
        return Err(failed.swap_remove(0).1);
    };
    let message = failed
        .iter()
        .map(|(id, error)| format!("{id}: {error:#}"))
        .collect::<Vec<_>>()
        .join("; ");
    let mut failure = Failure::new(first.exit, message).with_data(&rows);
    failure.hint.clone_from(&first.hint);
    failure.status = first.status;
    Err(failure.into())
}
