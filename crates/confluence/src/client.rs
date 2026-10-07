//! Confluence Cloud over REST: the config, the one door every request goes
//! through, paging for both API versions, display names, and row helpers.
//!
//! v2 (`/wiki/api/v2`) serves everything it has; v1 (`/wiki/rest/api`)
//! only search, the current user, user search, labels, attachment upload and
//! download, and moves: its content and space calls were removed and answer
//! 410 through the gateway. Both are reached through one of two bases: the
//! site with `Basic email:token` (a classic API token), or
//! `api.atlassian.com/ex/confluence/{cloud_id}` (a scoped token or a service
//! account). A credential goes to exactly that base's host: a suffix rule
//! for `atlassian.com` would also cover `api.media.atlassian.com`, where
//! attachment downloads redirect.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::config::{Section, Site};
use agent_cli_core::{
    Body, Credential, Ctx, Effect, Failure, Method, Op, Request, Response, Secret, base64,
    form_encode, host_under, status_of, utc,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};

pub(crate) const V2: &str = "/wiki/api/v2";
pub(crate) const V1: &str = "/wiki/rest/api";
const GATEWAY: &str = "api.atlassian.com";
/// Where an API token is made, for a 401's hint and doctor's.
pub(crate) const TOKEN_PAGE: &str = "https://id.atlassian.com/manage-profile/security/api-tokens";
/// v2 refuses a larger `limit`.
pub(crate) const PAGE_MAX: usize = 250;

/// One Confluence Cloud site, and the credential for it.
pub(crate) struct Confluence {
    pub(crate) site: Site,
    /// Where API calls go: the site, or the gateway with the cloud id.
    api: String,
    /// The one host the credential may go to.
    api_host: String,
    email: Option<String>,
    token: Option<Credential>,
    /// The `Authorization` value, made on the first request so `--dry-run`
    /// and read-only refusals never run a `token_cmd`.
    authorization: Mutex<Option<Secret>>,
}

impl Confluence {
    pub(crate) fn load(ctx: &Ctx) -> Result<Self> {
        let section: Section = ctx.section("confluence")?;
        let cloud_id = section
            .cloud_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty());
        if let Some(id) = cloud_id
            && !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        {
            return Err(
                Failure::setup(format!("[confluence] cloud_id {id:?} is not a cloud id"))
                    .hint("https://SITE.atlassian.net/_edge/tenant_info prints the site's cloudId")
                    .into(),
            );
        }
        let site = Site::parse(section.url.as_deref(), cloud_id.is_some())?;
        let (api, api_host) = match cloud_id {
            Some(id) => (
                format!("https://{GATEWAY}/ex/confluence/{id}"),
                GATEWAY.to_owned(),
            ),
            None => (site.web.clone(), site.host.clone()),
        };
        let email = section.email.filter(|email| !email.trim().is_empty());
        if email.is_none() && cloud_id.is_none() {
            return Err(Failure::setup(
                "[confluence] email is not set: a site's API token signs in as email:token",
            )
            .hint("set email to the address of the account the token belongs to")
            .into());
        }
        let token =
            Credential::from_keys("token", section.token, section.token_env, section.token_cmd)
                .map_err(|message| Failure::setup(message).hint(no_token_hint()))?;
        Ok(Self {
            site,
            api,
            api_host,
            email,
            token,
            authorization: Mutex::new(None),
        })
    }

    /// Where the token comes from and how it is sent, never what it is.
    pub(crate) fn source(&self) -> Option<String> {
        let token = self.token.as_ref()?;
        Some(match &self.email {
            Some(email) => format!("{} as {email} (Basic) to {}", token.source(), self.api_host),
            None => format!("{} (Bearer) to {}", token.source(), self.api_host),
        })
    }

    fn authorization(&self, ctx: &Ctx, fresh: bool) -> Result<Secret> {
        let mut held = self
            .authorization
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !fresh && let Some(held) = held.as_ref() {
            return Ok(held.clone());
        }
        let Some(token) = &self.token else {
            return Err(Failure::setup("no Confluence API token is set up")
                .hint(no_token_hint())
                .into());
        };
        let token = token.resolve(ctx)?;
        let value = match &self.email {
            Some(email) => format!(
                "Basic {}",
                base64(format!("{email}:{}", token.expose()).as_bytes())
            ),
            None => format!("Bearer {}", token.expose()),
        };
        let secret = Secret::new(value);
        *held = Some(secret.clone());
        Ok(secret)
    }

    pub(crate) fn v2(&self, path: &str, query: &[(&str, String)]) -> String {
        self.url(V2, path, query)
    }

    pub(crate) fn v1(&self, path: &str, query: &[(&str, String)]) -> String {
        self.url(V1, path, query)
    }

    fn url(&self, prefix: &str, path: &str, query: &[(&str, String)]) -> String {
        let mut url = format!("{}{prefix}{path}", self.api);
        if !query.is_empty() {
            url.push(if path.contains('?') { '&' } else { '?' });
            url.push_str(&encode(query));
        }
        url
    }

    /// A web link: a `_links.webui` path (relative to `/wiki`) on the site.
    pub(crate) fn web(&self, path: &str) -> String {
        format!("{}/wiki{path}", self.site.web)
    }

    pub(crate) fn web_of(&self, value: &Value) -> Option<String> {
        value["_links"]["webui"]
            .as_str()
            .or_else(|| value["webuiLink"].as_str())
            .map(|path| self.web(path))
    }

    pub(crate) fn get(&self, ctx: &Ctx, url: &str) -> Result<Value> {
        self.read(ctx, Call::new(Method::Get, url))?.json()
    }

    /// A `POST` that only reads (users-bulk).
    pub(crate) fn query(&self, ctx: &Ctx, url: &str, body: Value) -> Result<Value> {
        self.read(ctx, Call::new(Method::Query, url).json(body))?
            .json()
    }

    /// A change; an empty answer (a `204`) reads as `null`.
    pub(crate) fn change(
        &self,
        ctx: &Ctx,
        effect: Effect,
        method: Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut call = Call::new(method, url);
        if let Some(body) = body {
            call = call.json(body);
        }
        let response = self.write(ctx, effect, call)?;
        if response.body.trim().is_empty() {
            return Ok(Value::Null);
        }
        response.json()
    }

    pub(crate) fn read(&self, ctx: &Ctx, call: Call) -> Result<Response> {
        ctx.read(Signed {
            confluence: self,
            call,
        })
        .map_err(refused)
    }

    pub(crate) fn write(&self, ctx: &Ctx, effect: Effect, call: Call) -> Result<Response> {
        ctx.write(
            effect,
            Signed {
                confluence: self,
                call,
            },
        )
        .map_err(refused)
    }

    /// Every row of a v2 list, following `_links.next` (host-relative,
    /// `/wiki` included) until it is absent, never stopping at a short
    /// page; the rows up to `limit`, and whether there were more.
    pub(crate) fn list(&self, ctx: &Ctx, url: String, limit: usize) -> Result<(Vec<Value>, bool)> {
        let mut url = url;
        let mut rows = Vec::new();
        loop {
            let page = self.get(ctx, &url)?;
            rows.extend(page["results"].as_array().cloned().unwrap_or_default());
            let next = page["_links"]["next"]
                .as_str()
                .filter(|next| !next.is_empty());
            match next {
                Some(next) if rows.len() < limit => {
                    if !next.starts_with("/wiki/") {
                        bail!("Confluence answered a next link outside /wiki: {next}");
                    }
                    url = format!("{}{next}", self.api);
                }
                _ => {
                    let more = rows.len() > limit || next.is_some();
                    rows.truncate(limit);
                    return Ok((rows, more));
                }
            }
        }
    }

    /// Every row of a v1 search (`/search`, `/content/search`), following
    /// `_links.next`: relative to `/wiki`, and dropping parameters such as
    /// `excerpt`, which are added back. `totalSize` is an estimate.
    pub(crate) fn search(
        &self,
        ctx: &Ctx,
        path: &str,
        query: &[(&str, String)],
        limit: usize,
    ) -> Result<Searched> {
        let mut url = self.v1(path, query);
        let mut searched = Searched::default();
        loop {
            let page = self.get(ctx, &url)?;
            if searched.total.is_none() {
                searched.total = page["totalSize"].as_u64();
            }
            searched
                .rows
                .extend(page["results"].as_array().cloned().unwrap_or_default());
            let next = page["_links"]["next"]
                .as_str()
                .filter(|next| !next.is_empty());
            match next {
                Some(next) if searched.rows.len() < limit => {
                    if !next.starts_with('/') {
                        bail!("Confluence answered a next link that is not a path: {next}");
                    }
                    let mut next = format!("{}/wiki{next}", self.api);
                    for (key, value) in query {
                        if !has_param(&next, key) {
                            next.push('&');
                            next.push_str(&encode(&[(key, value.clone())]));
                        }
                    }
                    url = next;
                }
                _ => {
                    searched.more = searched.rows.len() > limit || next.is_some();
                    searched.rows.truncate(limit);
                    return Ok(searched);
                }
            }
        }
    }

    /// Display names by account id: one `users-bulk` per hundred ids. A
    /// refusal (an anonymous site cannot see profiles) leaves people as
    /// their ids rather than failing the read.
    pub(crate) fn names(&self, ctx: &Ctx, ids: &[String]) -> HashMap<String, String> {
        let mut wanted: Vec<&String> = Vec::new();
        for id in ids.iter().filter(|id| !id.is_empty()) {
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        let mut names = HashMap::new();
        // ponytail: 100 per call, v1 bulk's documented cap; v2 states none.
        for chunk in wanted.chunks(100) {
            let Ok(answer) = self.query(
                ctx,
                &self.v2("/users-bulk", &[]),
                json!({"accountIds": chunk}),
            ) else {
                break;
            };
            for user in answer["results"].as_array().into_iter().flatten() {
                if let (Some(id), Some(name)) = (
                    text(&user["accountId"]),
                    text(&user["displayName"]).or_else(|| text(&user["publicName"])),
                ) {
                    names.insert(id, name);
                }
            }
        }
        names
    }

    /// A space by its key (any case): v2 calls take its id, which a rename
    /// keeps, so the key is looked up each time.
    pub(crate) fn space(&self, ctx: &Ctx, key: &str) -> Result<Value> {
        let url = self.v2("/spaces", &[("keys", key.to_owned())]);
        let answer = self.get(ctx, &url)?;
        answer["results"]
            .as_array()
            .and_then(|spaces| spaces.first())
            .cloned()
            .ok_or_else(|| {
                Failure::not_found(format!("no space {key}, or you may not see it"))
                    .hint(format!(
                        "agent-cli confluence space list {}",
                        crate::ids::quote(key)
                    ))
                    .into()
            })
    }

    /// A space's key by its id, asked once per id a command meets.
    pub(crate) fn space_key(
        &self,
        ctx: &Ctx,
        id: &str,
        known: &mut HashMap<String, String>,
    ) -> Result<String> {
        if let Some(key) = known.get(id) {
            return Ok(key.clone());
        }
        let space = self.get(ctx, &self.v2(&format!("/spaces/{id}"), &[]))?;
        let key = text(&space["key"]).unwrap_or_else(|| id.to_owned());
        known.insert(id.to_owned(), key.clone());
        Ok(key)
    }

    /// A page or blog post by id (v2 keeps them apart: `/pages`, then
    /// `/blogposts`). v2 answers 404 for what is missing and for what the
    /// token may not see alike.
    pub(crate) fn content(&self, ctx: &Ctx, id: &str, query: &[(&str, String)]) -> Result<Content> {
        for kind in ["pages", "blogposts"] {
            match self.get(ctx, &self.v2(&format!("/{kind}/{id}"), query)) {
                Ok(value) => return Ok(Content { kind, value }),
                Err(error) if status_of(&error) == Some(404) => {}
                Err(error) => return Err(error),
            }
        }
        Err(
            Failure::not_found(format!("page {id} was not found, or you may not see it"))
                .hint("agent-cli confluence page list TEXT")
                .into(),
        )
    }
}

/// A page (`kind` `pages`) or blog post (`blogposts`) as v2 answered it.
pub(crate) struct Content {
    pub(crate) kind: &'static str,
    pub(crate) value: Value,
}

impl Content {
    /// `page` or `blogpost`, as search rows name them.
    pub(crate) fn type_name(&self) -> &'static str {
        if self.kind == "pages" {
            "page"
        } else {
            "blogpost"
        }
    }

    pub(crate) fn version(&self) -> u64 {
        self.value["version"]["number"].as_u64().unwrap_or(1)
    }
}

#[derive(Default)]
pub(crate) struct Searched {
    pub(crate) rows: Vec<Value>,
    pub(crate) total: Option<u64>,
    pub(crate) more: bool,
}

fn no_token_hint() -> String {
    format!(
        "make an API token at {TOKEN_PAGE} and set token_env = \"CONFLUENCE_TOKEN\" (or token_cmd) under [confluence]"
    )
}

/// A 401 names where tokens are made: every Cloud API token expires
/// within a year.
fn refused(mut error: anyhow::Error) -> anyhow::Error {
    if status_of(&error) == Some(401)
        && let Some(failure) = error.downcast_mut::<Failure>()
    {
        failure.hint = Some(format!(
            "the token was refused: it may have expired (Cloud API tokens do within a year) or belong to another email; make one at {TOKEN_PAGE}, then run agent-cli doctor confluence"
        ));
    }
    error
}

fn encode(query: &[(&str, String)]) -> String {
    let pairs: Vec<(String, String)> = query
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect();
    form_encode(&pairs)
}

fn has_param(url: &str, key: &str) -> bool {
    url.split_once('?').is_some_and(|(_, query)| {
        query
            .split('&')
            .any(|pair| pair.split('=').next() == Some(key))
    })
}

/// One request, before it is signed.
pub(crate) struct Call {
    method: Method,
    url: String,
    body: Body,
    headers: Vec<(String, String)>,
    keep_redirect: bool,
}

impl Call {
    pub(crate) fn new(method: Method, url: &str) -> Self {
        Self {
            method,
            url: url.to_owned(),
            body: Body::None,
            headers: Vec::new(),
            keep_redirect: false,
        }
    }

    pub(crate) fn json(mut self, body: Value) -> Self {
        self.body = Body::Json(body);
        self
    }

    pub(crate) fn bytes(mut self, bytes: Vec<u8>) -> Self {
        self.body = Body::Bytes(bytes);
        self
    }

    pub(crate) fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// A 3xx is the answer (an attachment's download redirect).
    pub(crate) fn keep_redirect(mut self) -> Self {
        self.keep_redirect = true;
        self
    }
}

/// A call with the credential, attached when it is performed: after core's
/// read-only and `--dry-run` checks, and only to the base's own host.
struct Signed<'a> {
    confluence: &'a Confluence,
    call: Call,
}

impl Signed<'_> {
    fn request<'r>(&self) -> Request<'r> {
        let mut request = Request::new(self.call.method, self.call.url.clone())
            .header("Accept", "application/json");
        for (name, value) in &self.call.headers {
            request = request.header(name.clone(), value.clone());
        }
        request.body = self.call.body.clone();
        if self.call.keep_redirect {
            request = request.keep_redirect();
        }
        request
    }
}

impl Op for Signed<'_> {
    type Output = Response;

    fn plan(&self) -> Value {
        let mut plan = self.request().plan();
        plan["headers"]["Authorization"] = json!("***");
        plan
    }

    fn writes(&self) -> bool {
        !self.call.method.is_read()
    }

    fn perform(self, ctx: &Ctx) -> Result<Response> {
        let host = &self.confluence.api_host;
        if !exactly(&self.call.url, host) {
            bail!(
                "refusing to send the Confluence credential to {}; it goes only to {host}",
                self.call.url
            );
        }
        let mint = |fresh: bool| self.confluence.authorization(ctx, fresh);
        self.request().auth(&mint).perform(ctx)
    }
}

/// True when `url` is `https://` to exactly `host`, nothing under it.
pub(crate) fn exactly(url: &str, host: &str) -> bool {
    host_under(url, host)
        && url
            .strip_prefix("https://")
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .is_some_and(|found| found.eq_ignore_ascii_case(host))
}

// ---------- rows ----------

/// A string field, `None` when absent or empty.
pub(crate) fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// A service's time as RFC 3339 UTC.
pub(crate) fn stamp(value: &Value) -> Option<String> {
    value.as_str().filter(|raw| !raw.is_empty()).map(utc)
}

/// The note a cut list prints: `[50 of 312; --limit N]`, `~` when the
/// total is an estimate, `50+` when it is unknown.
pub(crate) fn note_more(ctx: &Ctx, shown: usize, total: Option<String>, more: bool) {
    if more {
        let total = total.unwrap_or_else(|| format!("{shown}+"));
        ctx.note(format!("[{shown} of {total}; --limit N]"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credential_goes_to_exactly_one_host() {
        assert!(exactly(
            "https://api.atlassian.com/ex/confluence/1/wiki",
            "api.atlassian.com"
        ));
        assert!(!exactly(
            "https://api.media.atlassian.com/file/1",
            "api.atlassian.com"
        ));
        assert!(!exactly(
            "https://x.contoso.atlassian.net/wiki",
            "contoso.atlassian.net"
        ));
        assert!(!exactly(
            "http://contoso.atlassian.net/wiki",
            "contoso.atlassian.net"
        ));
    }

    #[test]
    fn a_cloud_id_sends_calls_and_next_links_through_the_gateway_as_bearer() {
        use agent_cli_core::Setup;
        use agent_cli_core::testing::{Answer, FakeTransport, run};

        let gateway =
            "https://api.atlassian.com/ex/confluence/11111111-2222-3333-4444-555555555555";
        let config = "[confluence]\nurl = \"https://contoso.atlassian.net/wiki\"\ncloud_id = \"11111111-2222-3333-4444-555555555555\"\ntoken_env = \"SCOPED\"\n";
        let transport = FakeTransport::answering([
            Answer::json(&json!({"results": [{"key": "ENG"}],
                "_links": {"next": "/wiki/api/v2/spaces?cursor=b", "base": "https://contoso.atlassian.net/wiki"}})),
            Answer::json(&json!({"results": [{"key": "OPS"}], "_links": {}})),
        ]);
        let setup = Setup::fake(transport.clone())
            .with_config(config)
            .with_env("SCOPED", "scoped-token-1");
        let outcome = run(&[crate::DOMAIN], &["confluence", "space", "list"], setup);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert!(
            sent[0]
                .url
                .starts_with(&format!("{gateway}/wiki/api/v2/spaces?"))
        );
        assert_eq!(
            sent[1].url,
            format!("{gateway}/wiki/api/v2/spaces?cursor=b")
        );
        assert_eq!(
            sent[0].authorization.as_deref(),
            Some("Bearer scoped-token-1")
        );
        // Web links name the site, not the gateway.
        assert_eq!(outcome.json()[0]["id"], "ENG");
    }

    #[test]
    fn a_v1_next_link_is_under_wiki_and_gets_back_what_it_dropped() {
        use crate::testing::{V1, confluence, urls};
        use agent_cli_core::testing::Answer;

        let first = json!({"results": [{"content": {"id": "1", "title": "a"}}], "totalSize": 9,
            "_links": {"next": "/rest/api/search?next=true&cursor=c2&limit=1&cql=x"}});
        let second = json!({"results": [{"content": {"id": "2", "title": "b"}}], "_links": {}});
        let (outcome, transport) = confluence(
            &["confluence", "page", "list", "--limit", "2"],
            vec![Answer::json(&first), Answer::json(&second)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{V1}/search?next=true&cursor=c2&limit=1&cql=x&expand=content.space%2Ccontent.version"
            )
        );
    }

    #[test]
    fn a_refused_token_names_where_tokens_are_made() {
        use crate::testing::confluence;
        use agent_cli_core::testing::Answer;

        let (outcome, _) = confluence(
            &["confluence", "space", "list"],
            vec![
                Answer::status(401, "Unauthorized"),
                Answer::status(401, "Unauthorized"),
            ],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(outcome.stderr.contains(TOKEN_PAGE), "{}", outcome.stderr);
    }
}
