//! Confluence Data Center and Server over REST: the config, the one door
//! every request goes through, paging, display names, and row helpers.
//!
//! Everything is v1 (`/rest/api`) under the site's base, context path
//! included, but for two things v1 has no call for: version history,
//! from `/rest/experimental`, and inline comments (create, reply, resolve),
//! from the inline-comments plugin's `/rest/inlinecomments/1.0`, which the
//! editor itself uses. A personal access token goes as `Bearer` to URLs
//! under the base and nowhere else.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::config::{Section, Site};
use agent_cli_core::{
    Body, Credential, Ctx, Effect, Failure, Method, Op, Request, Response, Secret, form_encode,
    percent_encode, same_origin, status_of, utc,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};

pub(crate) const API: &str = "/rest/api";
pub(crate) const INLINE: &str = "/rest/inlinecomments/1.0";
pub(crate) const EXPERIMENTAL: &str = "/rest/experimental";
/// Where a personal access token is made, below the base.
const TOKEN_PAGE: &str = "/plugins/personalaccesstokens/usertokens.action";
/// What a page read expands, the same in every command that reads a body,
/// so one recording answers them all.
pub(crate) const PAGE: &str = "body.storage,version,space,ancestors,metadata.labels,history";
/// The most rows a call asks for. The server lowers it without saying so
/// (to 50 for a search that expands bodies), so a short page never ends a
/// list.
pub(crate) const PAGE_MAX: usize = 200;

/// One self-hosted Confluence, and the token for it.
pub(crate) struct Confluence {
    pub(crate) site: Site,
    token: Option<Credential>,
    /// The `Authorization` value, made on the first request so `--dry-run`
    /// and `--yes` refusals never run a `token_cmd`.
    authorization: Mutex<Option<Secret>>,
}

impl Confluence {
    pub(crate) fn load(ctx: &Ctx) -> Result<Self> {
        let section: Section = ctx.section("confluence")?;
        let site = Site::parse(section.url.as_deref())?;
        let token =
            Credential::from_keys("token", section.token, section.token_env, section.token_cmd)
                .map_err(|message| Failure::setup(message).hint(no_token_hint(&site)))?;
        Ok(Self {
            site,
            token,
            authorization: Mutex::new(None),
        })
    }

    /// Where the token comes from and where it goes, never what it is.
    pub(crate) fn source(&self) -> Option<String> {
        let token = self.token.as_ref()?;
        Some(format!(
            "{} (Bearer) to {}/",
            token.source(),
            self.site.base
        ))
    }

    /// Where the person makes a token: their profile's token page.
    pub(crate) fn token_page(&self) -> String {
        format!("{}{TOKEN_PAGE}", self.site.base)
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
            return Err(
                Failure::setup("no Confluence personal access token is set up")
                    .hint(no_token_hint(&self.site))
                    .into(),
            );
        };
        let secret = Secret::new(format!("Bearer {}", token.resolve(ctx)?.expose()));
        *held = Some(secret.clone());
        Ok(secret)
    }

    /// A `/rest/api` URL.
    pub(crate) fn api(&self, path: &str, query: &[(&str, String)]) -> String {
        self.url(API, path, query)
    }

    /// A URL under another REST prefix (`INLINE`, `EXPERIMENTAL`).
    pub(crate) fn url(&self, prefix: &str, path: &str, query: &[(&str, String)]) -> String {
        let mut url = format!("{}{prefix}{path}", self.site.base);
        if !query.is_empty() {
            url.push(if path.contains('?') { '&' } else { '?' });
            url.push_str(&encode(query));
        }
        url
    }

    /// A web link: a `_links.webui` path, relative to the base.
    pub(crate) fn web(&self, path: &str) -> String {
        format!("{}{path}", self.site.base)
    }

    pub(crate) fn web_of(&self, value: &Value) -> Option<String> {
        value["_links"]["webui"].as_str().map(|path| self.web(path))
    }

    pub(crate) fn get(&self, ctx: &Ctx, url: &str) -> Result<Value> {
        self.read(ctx, Call::new(Method::Get, url))?.json()
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
        .map_err(|error| self.refused(error))
    }

    pub(crate) fn write(&self, ctx: &Ctx, effect: Effect, call: Call) -> Result<Response> {
        ctx.write(
            effect,
            Signed {
                confluence: self,
                call,
            },
        )
        .map_err(|error| self.refused(error))
    }

    /// Every row of a list, following `_links.next` (relative to the base)
    /// until it is absent, with any parameter it dropped added back; the
    /// rows up to `limit`, and whether there were more.
    pub(crate) fn list(&self, ctx: &Ctx, url: String, limit: usize) -> Result<(Vec<Value>, bool)> {
        let query = url.split_once('?').map(|(_, query)| query.to_owned());
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
                    if !next.starts_with("/rest/") {
                        bail!("Confluence answered a next link outside /rest: {next}");
                    }
                    let mut next = self.web(next);
                    for pair in query.iter().flat_map(|query| query.split('&')) {
                        let key = pair.split('=').next().unwrap_or_default();
                        if !key.is_empty() && !has_param(&next, key) {
                            next.push(if next.contains('?') { '&' } else { '?' });
                            next.push_str(pair);
                        }
                    }
                    url = next;
                }
                _ => {
                    let more = rows.len() > limit || next.is_some();
                    rows.truncate(limit);
                    return Ok((rows, more));
                }
            }
        }
    }

    /// Every row of a CQL `/search`, which answers no next link: pages by
    /// `start` until `totalSize` is reached, a page comes back empty or
    /// `limit` rows are in.
    pub(crate) fn search(
        &self,
        ctx: &Ctx,
        query: &[(&str, String)],
        limit: usize,
    ) -> Result<Searched> {
        let mut searched = Searched::default();
        let mut start = 0;
        loop {
            let mut asked = query.to_vec();
            asked.push(("limit", limit.clamp(1, PAGE_MAX).to_string()));
            if start > 0 {
                asked.push(("start", start.to_string()));
            }
            let page = self.get(ctx, &self.api("/search", &asked))?;
            let total = page["totalSize"].as_u64().map(|total| total as usize);
            searched.total = searched.total.or(total);
            let rows = page["results"].as_array().cloned().unwrap_or_default();
            start += rows.len();
            let done = rows.is_empty() || total.is_some_and(|total| start >= total);
            searched.rows.extend(rows);
            if done || searched.rows.len() >= limit {
                searched.more = searched.rows.len() > limit || !done;
                searched.rows.truncate(limit);
                return Ok(searched);
            }
        }
    }

    /// Display names by user key: one `/user?key=` per person. A refusal
    /// (a deleted user, a profile the token may not see) leaves people as
    /// their keys rather than failing the read.
    pub(crate) fn names(&self, ctx: &Ctx, keys: &[String]) -> HashMap<String, String> {
        let mut names = HashMap::new();
        for key in keys.iter().filter(|key| !key.is_empty()) {
            if names.contains_key(key) {
                continue;
            }
            let url = self.api("/user", &[("key", key.clone())]);
            match self.get(ctx, &url) {
                Ok(user) => {
                    if let Some(name) = person(&user) {
                        names.insert(key.clone(), name);
                    }
                }
                Err(error) if status_of(&error).is_some() => {}
                Err(_) => break,
            }
        }
        names
    }

    /// A space by its key, with its homepage.
    pub(crate) fn space(&self, ctx: &Ctx, key: &str) -> Result<Value> {
        let url = self.api(
            &format!("/space/{}", segment(key)),
            &[("expand", "homepage".to_owned())],
        );
        match self.get(ctx, &url) {
            Err(error) if status_of(&error) == Some(404) => Err(Failure::not_found(format!(
                "no space {key}, or you may not see it"
            ))
            .hint(format!(
                "agent-cli confluence space list {}",
                crate::ids::quote(key)
            ))
            .into()),
            found => found,
        }
    }

    /// A page or blog post by id, with what `expand` names, at `version`
    /// when given. Missing and not permitted are both a 404.
    pub(crate) fn content(
        &self,
        ctx: &Ctx,
        id: &str,
        expand: &str,
        version: Option<u64>,
    ) -> Result<Content> {
        let mut query = Vec::new();
        if !expand.is_empty() {
            query.push(("expand", expand.to_owned()));
        }
        if let Some(version) = version {
            query.push(("version", version.to_string()));
        }
        let value = match self.get(ctx, &self.api(&format!("/content/{id}"), &query)) {
            Ok(value) => value,
            Err(error) if status_of(&error) == Some(404) => {
                let what = match version {
                    Some(version) => {
                        format!("page {id} has no version {version}, or you may not see it")
                    }
                    None => format!("page {id} was not found, or you may not see it"),
                };
                let hint = match version {
                    Some(_) => format!("agent-cli confluence version list {id}"),
                    None => "agent-cli confluence page list TEXT".to_owned(),
                };
                return Err(Failure::not_found(what).hint(hint).into());
            }
            Err(error) => return Err(error),
        };
        let kind = text(&value["type"]).unwrap_or_default();
        if !matches!(kind.as_str(), "page" | "blogpost") {
            return Err(
                Failure::usage(format!("{id} is a {kind}, not a page or blog post"))
                    .hint("agent-cli confluence page list TEXT")
                    .into(),
            );
        }
        Ok(Content { kind, value })
    }

    /// A 401 names where tokens are made: they expire, and an admin can
    /// revoke them.
    fn refused(&self, mut error: anyhow::Error) -> anyhow::Error {
        if status_of(&error) == Some(401)
            && let Some(failure) = error.downcast_mut::<Failure>()
        {
            failure.hint = Some(format!(
                "the token was refused: it may have expired or been revoked; make one at {}, then run agent-cli doctor confluence",
                self.token_page()
            ));
        }
        error
    }
}

/// A page or blog post as v1 answered it.
pub(crate) struct Content {
    /// `page` or `blogpost`.
    pub(crate) kind: String,
    pub(crate) value: Value,
}

impl Content {
    pub(crate) fn version(&self) -> u64 {
        self.value["version"]["number"].as_u64().unwrap_or(1)
    }

    pub(crate) fn space(&self) -> Option<String> {
        text(&self.value["space"]["key"])
    }

    /// The parent page: the nearest of the ancestors, which run from the
    /// space's top down.
    pub(crate) fn parent(&self) -> Option<String> {
        self.value["ancestors"]
            .as_array()
            .and_then(|ancestors| ancestors.last())
            .and_then(|parent| text(&parent["id"]))
    }

    pub(crate) fn labels(&self) -> Vec<String> {
        self.value["metadata"]["labels"]["results"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|label| text(&label["name"]))
            .collect()
    }

    /// The storage body, when it was expanded.
    pub(crate) fn storage(&self) -> &str {
        self.value["body"]["storage"]["value"]
            .as_str()
            .unwrap_or_default()
    }
}

#[derive(Default)]
pub(crate) struct Searched {
    pub(crate) rows: Vec<Value>,
    pub(crate) total: Option<usize>,
    pub(crate) more: bool,
}

fn no_token_hint(site: &Site) -> String {
    format!(
        "make a personal access token at {}{TOKEN_PAGE} (your profile, Personal Access Tokens) and set token_env = \"CONFLUENCE_TOKEN\" (or token_cmd) under [confluence]",
        site.base
    )
}

/// A path segment, escaped: space keys may hold `~`, and personal ones
/// a username.
pub(crate) fn segment(raw: &str) -> String {
    let mut out = String::new();
    percent_encode(raw, &mut out);
    out.replace('+', "%20")
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
}

impl Call {
    pub(crate) fn new(method: Method, url: &str) -> Self {
        Self {
            method,
            url: url.to_owned(),
            body: Body::None,
            headers: Vec::new(),
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
}

/// A call with the token, attached when it is performed: after core's
/// `--dry-run` and `--yes` checks, and only under the site's base.
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
        let base = &self.confluence.site.base;
        if !same_origin(base, &self.call.url) {
            bail!(
                "refusing to send the Confluence token to {}; it goes only under {base}/",
                self.call.url
            );
        }
        let mint = |fresh: bool| self.confluence.authorization(ctx, fresh);
        self.request().auth(&mint).perform(ctx)
    }
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

/// A user object's name: `displayName`, else its username.
pub(crate) fn person(user: &Value) -> Option<String> {
    text(&user["displayName"]).or_else(|| text(&user["username"]))
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
    use agent_cli_core::testing::Answer;

    use super::*;
    use crate::testing::{API, BASE, confluence, urls};

    #[test]
    fn the_token_goes_only_under_the_base_as_bearer() {
        let (outcome, transport) = confluence(
            &["confluence", "space", "list"],
            vec![Answer::json(&json!({"results": [], "_links": {}}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert_eq!(
            sent[0].authorization.as_deref(),
            Some(format!("Bearer {}", crate::testing::TOKEN).as_str())
        );
        assert!(sent[0].url.starts_with(&format!("{API}/space?")));
        assert!(same_origin(BASE, &format!("{BASE}/rest/api/space")));
        assert!(!same_origin(BASE, "https://contoso.example/other/rest/api"));
        assert!(!same_origin(BASE, &format!("{BASE}x/rest/api")));
    }

    #[test]
    fn a_next_link_is_under_the_base_and_gets_back_what_it_dropped() {
        let first = json!({"results": [{"key": "ENG", "name": "Engineering"}],
            "_links": {"next": "/rest/api/space?limit=1&start=1", "base": BASE, "context": "/confluence"}});
        let second = json!({"results": [{"key": "OPS", "name": "Operations"}], "_links": {}});
        let (outcome, transport) = confluence(
            &["confluence", "space", "list"],
            vec![Answer::json(&first), Answer::json(&second)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport)[1],
            format!("{API}/space?limit=1&start=1&type=global&status=current&expand=homepage")
        );
        assert_eq!(outcome.json()[1]["id"], "OPS");
    }

    #[test]
    fn a_search_pages_by_start_up_to_its_total() {
        let page = |id: &str| {
            json!({"results": [{"content": {"id": id, "type": "page", "title": id}}],
                "start": 0, "size": 1, "totalSize": 2, "_links": {"base": BASE}})
        };
        let (outcome, transport) = confluence(
            &["confluence", "page", "list", "--limit", "5"],
            vec![Answer::json(&page("1")), Answer::json(&page("2"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json().as_array().unwrap().len(), 2);
        let sent = urls(&transport);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert!(sent[1].ends_with("&limit=5&start=1"), "{}", sent[1]);
        assert!(outcome.stderr.is_empty(), "{}", outcome.stderr);
    }

    #[test]
    fn a_refused_token_names_where_tokens_are_made() {
        let (outcome, _) = confluence(
            &["confluence", "space", "list"],
            vec![
                Answer::status(401, "Unauthorized"),
                Answer::status(401, "Unauthorized"),
            ],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains(&format!(
                "{BASE}/plugins/personalaccesstokens/usertokens.action"
            )),
            "{}",
            outcome.stderr
        );
    }
}
