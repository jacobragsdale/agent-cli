//! Azure DevOps over REST: the credential, the one door every request goes
//! through, and the lookups worth caching for a day (who "me" is, repository
//! and pipeline ids). Where the organization and projects come from is
//! `config.rs`.
//!
//! Ported from ticket-tui's `azure.rs`. Retrying a spent token, waiting out a
//! throttle and refusing redirects are core's; what is Azure DevOps's own is
//! here: the sign-in page it answers bad credentials with, the headers and
//! API versions it wants, and where a credential may be sent.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_cli_core::{
    Credential, Ctx, Effect, Failure, Method, Request, Response, Secret, percent_encode,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::{SIGN_IN_HINT, refused_sign_in, signed_out, trusted};

pub(crate) const API: &str = "7.1";
/// Work item comments are still behind a preview flag on every 7.x version.
pub(crate) const COMMENTS_API: &str = "7.1-preview.4";
/// Connection data refuses a plain `7.1` outright.
const CONNECTION_DATA_API: &str = "7.1-preview";
/// Approvals and policy evaluations only exist as previews.
pub(crate) const PREVIEW_API: &str = "7.1-preview.1";
/// IDs change about never; a day keeps a renamed repository from lingering.
const CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

/// One organization and its projects, and the credential for them.
#[derive(Clone)]
pub(crate) struct Ado {
    pub(crate) org: String,
    /// The project this command works in: `--project`, the work item's own,
    /// else the first of `[ado] project`. Work URLs and cache keys follow
    /// it, since two projects may run different processes.
    pub(crate) project: String,
    /// `[ado] project`: every project a work item list searches.
    pub(crate) projects: Vec<String>,
    pub(crate) code_project: String,
    /// `project`'s teams in `[ado] team`.
    pub(crate) teams: Vec<String>,
    /// `[ado] team` as (project, team).
    pub(crate) all_teams: Vec<(String, String)>,
    /// A personal access token from `pat_env`, `pat_cmd` or
    /// `AZURE_DEVOPS_EXT_PAT`; `None` borrows `az`'s login.
    pub(crate) pat: Option<Credential>,
    /// The PAT as `Basic …`, read on the first request and again only when
    /// a refused `pat_kv` changed in Key Vault; shared by every copy.
    pub(crate) basic: Arc<Mutex<Option<Secret>>>,
}

impl Ado {
    /// A read.
    pub(crate) fn get(&self, ctx: &Ctx, url: &str) -> Result<Value> {
        self.send(ctx, Method::Get, url, Body::None, None)?.json()
    }

    /// A `POST` that only reads: WIQL, a work item batch.
    pub(crate) fn query(&self, ctx: &Ctx, url: &str, body: Value) -> Result<Value> {
        self.send(ctx, Method::Query, url, Body::Json(body), None)?
            .json()
    }

    /// A change, which core checks against `--dry-run` and `--yes`. Some
    /// changes answer with no body; that reads as `null`.
    pub(crate) fn change(
        &self,
        ctx: &Ctx,
        effect: Effect,
        method: Method,
        url: &str,
        body: Value,
    ) -> Result<Value> {
        let response = self.send(ctx, method, url, Body::Json(body), Some(effect))?;
        if response.body.trim().is_empty() {
            return Ok(Value::Null);
        }
        response.json()
    }

    /// A JSON Patch document to a work item: `PATCH` to change one, `POST`
    /// to create one.
    pub(crate) fn patch_work_item(
        &self,
        ctx: &Ctx,
        method: Method,
        url: &str,
        document: Vec<Value>,
    ) -> Result<Value> {
        let body = Body::Patch(Value::Array(document));
        self.send(ctx, method, url, body, Some(Effect::Write))?
            .json()
    }

    /// The one door: the headers Azure DevOps wants, the credential only for
    /// its own hosts, and its ways of saying "sign in again" read as exit 3.
    /// Core retries a 401 with a fresh credential; a sign-in page or redirect
    /// refusing a PAT gets the same one retry here, when a `pat_kv` changed.
    pub(crate) fn send(
        &self,
        ctx: &Ctx,
        method: Method,
        url: &str,
        body: Body,
        effect: Option<Effect>,
    ) -> Result<Response> {
        let spare = self.pat.is_some().then(|| body.clone());
        let error = match self.send_once(ctx, method, url, body, effect) {
            Err(error) if refused_sign_in(&error) => error,
            answer => return answer,
        };
        match spare {
            Some(body) if self.renew(ctx)? => self.send_once(ctx, method, url, body, effect),
            _ => Err(error),
        }
    }

    fn send_once(
        &self,
        ctx: &Ctx,
        method: Method,
        url: &str,
        body: Body,
        effect: Option<Effect>,
    ) -> Result<Response> {
        if !trusted(url) {
            bail!("refusing to send the Azure DevOps credential to {url}");
        }
        let mint = |fresh: bool| self.authorization(ctx, fresh);
        let mut request = Request::new(method, url)
            .header("Accept", "application/json")
            .header("X-VSS-ForceMsaPassThrough", "true")
            .auth(&mint);
        match body {
            Body::None => {}
            Body::Json(body) => request = request.json(body),
            // Azure DevOps refuses a patch document sent as plain JSON.
            Body::Patch(body) => {
                request = request
                    .json(body)
                    .header("Content-Type", "application/json-patch+json");
            }
            Body::Bytes(bytes) => request = request.bytes(bytes),
        }
        let response = match effect {
            None => ctx.read(request),
            Some(effect) => ctx.write(effect, request),
        }
        .map_err(signed_out)
        .map_err(|error| self.no_such_project(error))?;
        // Bad credentials can also come back as a 203 sign-in page: a success
        // status carrying HTML, which would otherwise read as broken JSON.
        if response.status == 203 {
            return Err(Failure::setup(format!(
                "{} {url} answered 203 with a sign-in page: Azure DevOps refused the credential",
                method.wire()
            ))
            .hint(SIGN_IN_HINT)
            .into());
        }
        Ok(response)
    }

    /// A project the organization lacks (TF200016), from `--project` or a
    /// file id: exit 2 naming the configured ones.
    fn no_such_project(&self, error: anyhow::Error) -> anyhow::Error {
        match error.downcast_ref::<Failure>() {
            Some(failure) if failure.message.contains("TF200016") => {
                Failure::usage(failure.message.clone())
                    .hint(format!(
                        "pass --project one of [ado] project: {}",
                        self.projects.join(", ")
                    ))
                    .into()
            }
            _ => error,
        }
    }

    /// `https://dev.azure.com/{org}`.
    pub(crate) fn base(&self) -> String {
        format!("https://dev.azure.com/{}", segment(&self.org))
    }

    /// `{org}/[{project}/]_apis/{path}?{query}&api-version={version}`. `path`
    /// and `query` arrive escaped.
    pub(crate) fn api(
        &self,
        project: Option<&str>,
        path: &str,
        query: &str,
        version: &str,
    ) -> String {
        let scope = project.map(|project| vec![project]).unwrap_or_default();
        self.scoped(&scope, path, query, version)
    }

    /// A team's own endpoints hang off `{org}/{project}/{team}/_apis`.
    pub(crate) fn team(&self, team: &str, path: &str, query: &str) -> String {
        self.scoped(&[&self.project, team], path, query, API)
    }

    fn scoped(&self, scope: &[&str], path: &str, query: &str, version: &str) -> String {
        let mut url = self.base();
        for part in scope {
            url.push('/');
            url.push_str(&segment(part));
        }
        url.push_str("/_apis/");
        url.push_str(path);
        url.push('?');
        if !query.is_empty() {
            url.push_str(query);
            url.push('&');
        }
        url.push_str("api-version=");
        url.push_str(version);
        url
    }

    /// Under the work items' project.
    pub(crate) fn work(&self, path: &str, query: &str) -> String {
        self.api(Some(&self.project), path, query, API)
    }

    /// Under the code project: Git, pull requests, builds, pipelines.
    pub(crate) fn code(&self, path: &str, query: &str) -> String {
        self.api(Some(&self.code_project), path, query, API)
    }

    /// Under the project the item is in (ids are the organization's), else
    /// the configured one.
    pub(crate) fn work_item_url(&self, project: Option<&str>, id: i64) -> String {
        format!(
            "{}/{}/_workitems/edit/{id}",
            self.base(),
            segment(project.unwrap_or(&self.project))
        )
    }

    /// Where the web UI shows a pull request, which the list endpoint does not
    /// always say.
    pub(crate) fn pull_request_url(&self, project: &str, repo: &str, id: i64) -> String {
        format!(
            "{}/{}/_git/{}/pullrequest/{id}",
            self.base(),
            segment(project),
            segment(repo)
        )
    }

    /// A work item, pull request or run id as an agent was handed it:
    /// `1207`, `#1207`, `AB#1207`, or its web URL in this organization. A URL
    /// is read, never fetched; one in another organization, or naming another
    /// kind of thing, is exit 2.
    pub(crate) fn id(&self, kind: Kind, raw: &str) -> Result<i64> {
        let raw = raw.trim();
        let wrong = |why: String| -> anyhow::Error {
            Failure::usage(why)
                .hint(format!(
                    "pass the {}'s number, e.g. 1207, #1207 or its {} URL",
                    kind.noun(),
                    self.base()
                ))
                .into()
        };
        let Some((segments, query)) = crate::ids::web(self, raw).map_err(wrong)? else {
            let number = raw
                .strip_prefix("AB#")
                .or_else(|| raw.strip_prefix('#'))
                .unwrap_or(raw);
            return number
                .parse::<i64>()
                .ok()
                .filter(|id| (1..=i64::from(i32::MAX)).contains(id))
                .ok_or_else(|| wrong(format!("{raw:?} is not a {} id", kind.noun())));
        };
        let after = |marker: &str| {
            segments
                .iter()
                .position(|segment| segment.eq_ignore_ascii_case(marker))
                .and_then(|at| segments.get(at + 1))
                .and_then(|id| id.parse::<i64>().ok())
        };
        let found = match kind {
            Kind::WorkItem => after("edit").filter(|_| segments.iter().any(|s| s == "_workitems")),
            Kind::PullRequest => after("pullrequest"),
            Kind::Run => crate::ids::query_param(&query, "buildId").and_then(|id| id.parse().ok()),
        };
        found.ok_or_else(|| wrong(format!("{raw} is not a {} URL", kind.noun())))
    }

    pub(crate) fn cache_key(&self, what: &str) -> String {
        format!("ado:{}:{what}", self.org.to_ascii_lowercase())
    }

    /// Who the credential signs in as. Votes and auto-complete are written
    /// under this id, and `@me` assigns to its sign-in address.
    // ponytail: cached per organization, not per credential; after signing in
    // as someone else, --no-cache (or a day) catches up.
    pub(crate) fn me(&self, ctx: &Ctx) -> Result<Person> {
        match ctx.cache().get(&self.cache_key("me")) {
            Some(me) => Ok(me),
            None => self.fetch_me(ctx),
        }
    }

    /// [`Self::me`] asked of Azure DevOps now, which is also what doctor's
    /// connection check is.
    pub(crate) fn fetch_me(&self, ctx: &Ctx) -> Result<Person> {
        let data = self.get(
            ctx,
            &self.api(None, "connectionData", "", CONNECTION_DATA_API),
        )?;
        let user = &data["authenticatedUser"];
        let me = Person {
            id: user["id"]
                .as_str()
                .context("Azure DevOps did not say who is signed in")?
                .to_owned(),
            name: text(&user["providerDisplayName"])
                .or_else(|| text(&user["customDisplayName"]))
                .unwrap_or_default(),
            email: text(&user["properties"]["Account"]["$value"]),
        };
        ctx.cache().put(&self.cache_key("me"), &me, CACHE_TTL);
        Ok(me)
    }

    /// The identity id a name, an address or `@me` stands for, which is what
    /// the pull request search filters by.
    pub(crate) fn identity(&self, ctx: &Ctx, who: &str) -> Result<String> {
        Ok(self.person(ctx, who)?.id)
    }

    /// The one person a name, an address or `@me` stands for: exit 4 for
    /// nobody, exit 2 for several, so nothing is assigned or mentioned on a
    /// guess.
    pub(crate) fn person(&self, ctx: &Ctx, who: &str) -> Result<Person> {
        if who.eq_ignore_ascii_case("@me") {
            return self.me(ctx);
        }
        let key = self.cache_key(&format!("person:{}", who.to_lowercase()));
        if let Some(person) = ctx.cache().get(&key) {
            return Ok(person);
        }
        let url = format!(
            "https://vssps.dev.azure.com/{}/_apis/identities?searchFilter=General&filterValue={}&queryMembership=None&api-version={API}",
            segment(&self.org),
            query_value(who)
        );
        let found = self.get(ctx, &url)?;
        // (the person, whether a name or address is exactly `who`)
        let people: Vec<(Person, bool)> = list(&found["value"])
            .iter()
            .filter_map(|identity| {
                let properties = &identity["properties"];
                let names = [
                    &identity["providerDisplayName"],
                    &properties["Mail"]["$value"],
                    &properties["Account"]["$value"],
                ];
                let exact = names.iter().any(|name| {
                    name.as_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case(who))
                });
                let person = Person {
                    id: identity["id"].as_str()?.to_owned(),
                    name: text(names[0]).unwrap_or_default(),
                    email: text(names[2]).or_else(|| text(names[1])),
                };
                Some((person, exact))
            })
            .collect();
        let exact: Vec<&(Person, bool)> = people.iter().filter(|person| person.1).collect();
        let chosen = match (exact.as_slice(), people.as_slice()) {
            ([one], _) => *one,
            (_, [one]) => one,
            (_, []) => {
                return Err(
                    Failure::not_found(format!("nobody in {} matches {who:?}", self.org))
                        .hint("agent-cli ado person list --text NAME (or give a full sign-in address, such as jane@contoso.com)")
                        .into(),
                );
            }
            // Several exact ones (two people sharing a display name) are as
            // ambiguous as several partial ones.
            (exact, many) => {
                let many: Vec<&Person> = match exact {
                    [] => many.iter().map(|person| &person.0).collect(),
                    exact => exact.iter().map(|person| &person.0).collect(),
                };
                let mut names: Vec<String> = (many.iter().take(10))
                    .map(|person| match &person.email {
                        Some(email) => format!("{} <{email}>", person.name),
                        None => person.name.clone(),
                    })
                    .collect();
                if many.len() > names.len() {
                    names.push(format!("and {} more", many.len() - names.len()));
                }
                return Err(Failure::usage(format!(
                    "{who:?} matches {} people: {}",
                    many.len(),
                    names.join(", ")
                ))
                .hint("give their sign-in address instead")
                .into());
            }
        };
        ctx.cache().put(&key, &chosen.0, CACHE_TTL);
        Ok(chosen.0.clone())
    }

    /// The code project's repositories, from the cache unless `fresh`.
    pub(crate) fn repos(&self, ctx: &Ctx, fresh: bool) -> Result<Vec<RepoRef>> {
        let key = self.cache_key(&format!("repos:{}", self.code_project.to_lowercase()));
        if !fresh && let Some(repos) = ctx.cache().get(&key) {
            return Ok(repos);
        }
        let answer = self.get(ctx, &self.code("git/repositories", ""))?;
        let repos: Vec<RepoRef> = list(&answer["value"])
            .iter()
            .filter_map(RepoRef::parse)
            .collect();
        ctx.cache().put(&key, &repos, CACHE_TTL);
        Ok(repos)
    }

    /// One repository by name or id, read live: its id changes when it is
    /// made again, and its default branch with a first push or a setting.
    pub(crate) fn repo(&self, ctx: &Ctx, name: &str) -> Result<RepoRef> {
        let url = self.code(&format!("git/repositories/{}", segment(name.trim())), "");
        let answer = self.get(ctx, &url).map_err(|error| {
            if agent_cli_core::status_of(&error) == Some(404) {
                Failure::not_found(format!(
                    "there is no repository {name:?} in {}",
                    self.code_project
                ))
                .hint("agent-cli ado repo list --fields name")
                .into()
            } else {
                error
            }
        })?;
        RepoRef::parse(&answer).context("the repository came back without its id and name")
    }

    /// A pipeline's id: as given when it is a number, else looked up by name,
    /// live: a pipeline made again under its name has a new id.
    pub(crate) fn pipeline_id(&self, ctx: &Ctx, name: &str) -> Result<i64> {
        if let Ok(id) = name.trim().parse() {
            return Ok(id);
        }
        let answer = self.get(
            ctx,
            &self.code("build/definitions", &format!("name={}", query_value(name))),
        )?;
        let found: Vec<(i64, String)> = list(&answer["value"])
            .iter()
            .filter(|definition| {
                definition["name"]
                    .as_str()
                    .is_some_and(|held| held.eq_ignore_ascii_case(name))
            })
            .filter_map(|definition| {
                Some((
                    definition["id"].as_i64()?,
                    text(&definition["path"]).unwrap_or_default(),
                ))
            })
            .collect();
        let id = match found.as_slice() {
            [(id, _)] => *id,
            [] => {
                return Err(Failure::not_found(format!(
                    "there is no pipeline {name:?} in {}",
                    self.code_project
                ))
                .hint(format!(
                    "agent-cli ado pipeline list {} --fields id,name",
                    crate::ids::arg(name)
                ))
                .into());
            }
            many => {
                let named: Vec<String> = many
                    .iter()
                    .map(|(id, folder)| format!("{folder}\\{name} ({id})"))
                    .collect();
                return Err(Failure::usage(format!(
                    "{name:?} names {} pipelines: {}",
                    many.len(),
                    named.join(", ")
                ))
                .hint("pass the pipeline's id instead")
                .into());
            }
        };
        Ok(id)
    }
}

/// What an id argument names.
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    WorkItem,
    PullRequest,
    Run,
}

impl Kind {
    fn noun(self) -> &'static str {
        match self {
            Self::WorkItem => "work item",
            Self::PullRequest => "pull request",
            Self::Run => "run",
        }
    }
}

/// What a request carries.
#[derive(Clone)]
pub(crate) enum Body {
    None,
    Json(Value),
    /// A JSON Patch document, which Azure DevOps only takes under its own
    /// media type.
    Patch(Value),
    /// A file's bytes, sent as `application/octet-stream`.
    Bytes(Vec<u8>),
}

/// Someone `--assignee`, a mention or a reviewer names, or who signed in.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Person {
    /// The identity id, which reviewers and the pull request search take.
    pub(crate) id: String,
    pub(crate) name: String,
    /// The sign-in address (else their mail), which an assignment is
    /// written with. `account` is what a day-old cached `me` calls it.
    #[serde(alias = "account")]
    pub(crate) email: Option<String>,
}

/// What the writes need to know about a repository.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RepoRef {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) default_branch: Option<String>,
    /// The project's GUID, which artifact links name.
    pub(crate) project_id: Option<String>,
}

impl RepoRef {
    fn parse(entry: &Value) -> Option<Self> {
        Some(Self {
            id: entry["id"].as_str()?.to_owned(),
            name: entry["name"].as_str()?.to_owned(),
            default_branch: text(&entry["defaultBranch"]),
            project_id: text(&entry["project"]["id"]),
        })
    }
}

/// A URL path segment: project and repository names carry spaces.
pub(crate) fn segment(raw: &str) -> String {
    // percent_encode writes a space as `+`, which is only a space in a query;
    // every literal `+` it was given comes out as `%2B`.
    query_value(raw).replace('+', "%20")
}

/// A query-string value.
pub(crate) fn query_value(raw: &str) -> String {
    let mut out = String::new();
    percent_encode(raw, &mut out);
    out
}

/// A non-empty string, owned.
pub(crate) fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// A timestamp as every command prints one: RFC 3339, UTC, whole seconds.
pub(crate) fn stamp(value: &Value) -> Option<String> {
    text(value).map(|raw| agent_cli_core::utc(&raw))
}

/// A JSON array's items, or none.
pub(crate) fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

/// `main`, not `refs/heads/main`.
pub(crate) fn short_branch(reference: &str) -> String {
    reference
        .strip_prefix("refs/heads/")
        .unwrap_or(reference)
        .to_owned()
}

/// `refs/heads/main`, however it was typed.
pub(crate) fn full_ref(branch: &str) -> String {
    let branch = branch.trim();
    if branch.starts_with("refs/") {
        branch.to_owned()
    } else {
        format!("refs/heads/{branch}")
    }
}

/// How long a successful answer asks the next request to hold off. Azure
/// DevOps reports a spent budget with `X-RateLimit-Remaining: 0` (until the
/// `X-RateLimit-Reset` epoch) ahead of the 429 it turns into, and sends
/// `Retry-After` on answers it delayed.
pub(crate) fn rate_limit_pause(response: &Response) -> Option<Duration> {
    let number = |name: &str| response.header(name)?.trim().parse::<f64>().ok();
    let seconds = if let Some(after) = number("Retry-After") {
        after
    } else if number("X-RateLimit-Remaining")? <= 0.0 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64());
        number("X-RateLimit-Reset").map_or(30.0, |reset| reset - now)
    } else {
        return None;
    };
    (seconds >= 1.0).then(|| Duration::from_secs_f64(seconds.min(3600.0)))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::auth::basic;
    use crate::config::Section;
    use crate::{DOMAIN, testing};
    use agent_cli_core::testing::run;
    use agent_cli_core::testing::{Answer, FakeTransport, ctx};
    use agent_cli_core::{Config, Exit, Setup};
    use serde_json::json;

    use super::*;

    fn section(toml: &str) -> Section {
        Config::parse("c.toml", Some(toml), Vec::new())
            .section("ado")
            .unwrap()
    }

    #[test]
    fn a_pat_goes_as_basic_and_segments_escape_spaces_and_plus() {
        assert_eq!(basic("abc").expose(), "Basic OmFiYw==");
        assert_eq!(segment("Web Team+1/x"), "Web%20Team%2B1%2Fx");
        assert_eq!(query_value("a b&c"), "a+b%26c");
    }

    #[test]
    fn the_credential_goes_only_to_azure_devops_hosts() {
        for url in [
            "https://dev.azure.com/contoso/_apis/projects",
            "https://vssps.dev.azure.com/contoso/_apis/identities",
            "https://contoso.visualstudio.com/_apis/x",
        ] {
            assert!(trusted(url), "{url}");
        }
        for url in [
            "https://dev.azure.com.evil.example/",
            "https://visualstudio.com/",
            "http://dev.azure.com/contoso",
            "https://login.microsoftonline.com/",
        ] {
            assert!(!trusted(url), "{url}");
        }
        let transport = FakeTransport::default();
        let ctx = ctx(Setup::fake(transport.clone()));
        let ado = Ado::resolve(
            section("[ado]\norg=\"contoso\"\nproject=\"Fabrikam\"\n"),
            || (None, None),
            Path::new("c"),
        )
        .unwrap();
        let error = ado.get(&ctx, "https://evil.example/_apis/x").unwrap_err();
        assert!(error.to_string().contains("refusing"), "{error}");
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn an_id_is_a_number_a_hash_or_a_web_url_in_this_organization() {
        let ado = Ado::resolve(
            section("[ado]\norg=\"contoso\"\nproject=\"Fabrikam\"\n"),
            || (None, None),
            Path::new("c"),
        )
        .unwrap();
        for (kind, raw, want) in [
            (Kind::WorkItem, "1207", 1207),
            (Kind::WorkItem, "#1207", 1207),
            (Kind::WorkItem, " AB#1207 ", 1207),
            (
                Kind::WorkItem,
                "https://dev.azure.com/contoso/Fabrikam/_workitems/edit/1207/",
                1207,
            ),
            (
                Kind::WorkItem,
                "https://contoso.visualstudio.com/Fabrikam/_workitems/edit/1207",
                1207,
            ),
            (
                Kind::PullRequest,
                "https://dev.azure.com/Contoso/Fabrikam/_git/api/pullrequest/431?_a=files",
                431,
            ),
            (
                Kind::Run,
                "https://dev.azure.com/contoso/Fabrikam/_build/results?buildId=8812&view=logs",
                8812,
            ),
        ] {
            assert_eq!(ado.id(kind, raw).unwrap(), want, "{raw}");
        }
        for (kind, raw, want) in [
            (Kind::WorkItem, "twelve", "\"twelve\" is not a work item id"),
            (Kind::Run, "0", "\"0\" is not a run id"),
            (
                Kind::Run,
                "https://dev.azure.com/fabrikam/Web/_build/results?buildId=1",
                "is in organization fabrikam, and [ado] org is contoso",
            ),
            (
                Kind::Run,
                "https://dev.azure.com/contoso/Fabrikam/_git/api/pullrequest/431",
                "is not a run URL",
            ),
            (
                Kind::PullRequest,
                "https://evil.example/pullrequest/4",
                "is not an Azure DevOps URL",
            ),
        ] {
            let error = ado.id(kind, raw).unwrap_err();
            let failure = error.downcast_ref::<Failure>().unwrap();
            assert_eq!(failure.exit, Exit::Usage);
            assert!(failure.message.contains(want), "{}", failure.message);
        }
    }

    #[test]
    fn a_sign_in_page_or_a_redirect_is_needs_setup_with_the_ado_hint() {
        let ado = Ado {
            pat: Credential::from_keys("pat", Some("fixture-pat".into()), None, None, None)
                .unwrap(),
            ..Ado::resolve(
                section("[ado]\norg=\"contoso\"\nproject=\"Fabrikam\"\n"),
                || (None, None),
                Path::new("c"),
            )
            .unwrap()
        };
        for answer in [
            Answer::status(203, "<html>Sign in</html>"),
            Answer::status(302, "").with_header(
                "Location",
                "https://spsprodcus1.vssps.visualstudio.com/_signin",
            ),
            Answer::status(401, "{}"),
        ] {
            let transport = FakeTransport::answering([answer.clone(), answer]);
            let ctx = ctx(Setup::fake(transport.clone()));
            let error = ado.get(&ctx, &ado.work("wit/workitems/1", "")).unwrap_err();
            let failure = error.downcast_ref::<Failure>().unwrap();
            assert_eq!(failure.exit, Exit::Setup, "{failure:?}");
            assert!(
                failure
                    .hint
                    .as_deref()
                    .unwrap()
                    .contains("AZURE_DEVOPS_EXT_PAT")
            );
            let sent = transport.sent();
            assert_eq!(
                sent[0].authorization.as_deref(),
                Some("Basic OmZpeHR1cmUtcGF0")
            );
        }
    }

    #[test]
    fn a_spent_rate_limit_budget_or_retry_after_asks_for_a_pause() {
        let with = |headers: &[(&str, &str)]| Response {
            status: 200,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            ..Response::default()
        };
        assert_eq!(
            rate_limit_pause(&with(&[("Retry-After", "7")])),
            Some(Duration::from_secs(7))
        );
        assert_eq!(
            rate_limit_pause(&with(&[("X-RateLimit-Remaining", "12")])),
            None
        );
        let soon = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 20;
        let pause = rate_limit_pause(&with(&[
            ("X-RateLimit-Remaining", "0"),
            ("X-RateLimit-Reset", &soon.to_string()),
        ]))
        .unwrap();
        assert!(
            pause > Duration::from_secs(15) && pause <= Duration::from_secs(20),
            "{pause:?}"
        );
        assert_eq!(rate_limit_pause(&with(&[])), None);
    }

    #[test]
    fn lookups_are_cached_and_an_ambiguous_name_is_a_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        let transport = FakeTransport::answering([
            Answer::json(
                &json!({"authenticatedUser": {"id": "u-1", "providerDisplayName": "Jane Doe",
                "properties": {"Account": {"$type": "System.String", "$value": "jane@contoso.com"}}}}),
            ),
            Answer::json(&json!({"count": 2, "value": [
                {"id": "u-2", "providerDisplayName": "Sam Lee", "properties": {"Mail": {"$value": "sam@contoso.com"}}},
                {"id": "u-3", "providerDisplayName": "Sam Leeds", "properties": {}}
            ]})),
            Answer::json(&json!({"count": 2, "value": [
                {"id": "u-2", "providerDisplayName": "Sam Lee", "properties": {}},
                {"id": "u-3", "providerDisplayName": "Sam Leeds", "properties": {}}
            ]})),
        ]);
        let setup = Setup {
            cache_dir: Some(dir.path().to_owned()),
            ..Setup::fake(transport.clone())
        };
        let ctx = ctx(setup);
        let ado = Ado {
            pat: Credential::from_keys("pat", Some("fixture-pat".into()), None, None, None)
                .unwrap(),
            ..Ado::resolve(
                section("[ado]\norg=\"contoso\"\nproject=\"Fabrikam\"\n"),
                || (None, None),
                Path::new("c"),
            )
            .unwrap()
        };
        assert_eq!(
            ado.me(&ctx).unwrap().email.as_deref(),
            Some("jane@contoso.com")
        );
        assert_eq!(
            ado.identity(&ctx, "@ME").unwrap(),
            "u-1",
            "cached: no second call"
        );
        assert_eq!(ado.identity(&ctx, "sam@contoso.com").unwrap(), "u-2");
        assert_eq!(
            ado.identity(&ctx, "SAM@contoso.com").unwrap(),
            "u-2",
            "cached"
        );
        let sam = ado.person(&ctx, "sam@contoso.com").unwrap();
        assert_eq!(
            (sam.name.as_str(), sam.email.as_deref()),
            ("Sam Lee", Some("sam@contoso.com")),
            "cached as a whole person"
        );
        let error = ado.identity(&ctx, "Sam").unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.exit, Exit::Usage);
        assert!(
            failure.message.contains("Sam Lee, Sam Leeds"),
            "{}",
            failure.message
        );
        assert_eq!(transport.sent().len(), 3);
        assert!(
            transport.sent()[0]
                .url
                .ends_with("/_apis/connectionData?api-version=7.1-preview")
        );
        assert!(transport.sent()[1].url.starts_with("https://vssps.dev.azure.com/contoso/_apis/identities?searchFilter=General&filterValue=sam%40contoso.com"));
        let stored = std::fs::read_to_string(dir.path().join("cache.json")).unwrap();
        assert!(!stored.contains("fixture-pat") && !stored.contains("Basic"));
    }

    #[test]
    fn nobody_matching_is_not_found_and_points_to_person_list() {
        let (outcome, _) = testing::ado(
            &["ado", "pr", "list", "--author", "Nobody Here"],
            vec![testing::page(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli ado person list --text NAME (or give"),
            "{}",
            outcome.stderr
        );
    }

    /// Runs `workitem get 1` with `pat_kv` against these answers: what it
    /// exited with, and each request's URL and `Authorization`.
    fn with_pat_kv(answers: Vec<Answer>) -> (u8, Vec<(String, Option<String>)>) {
        let transport = FakeTransport::answering(answers);
        let config = format!("{}pat_kv = \"kv-contoso/ado-pat\"\n", testing::CONFIG);
        let setup = Setup::fake(transport.clone()).with_config(&config);
        let outcome = run(
            &[DOMAIN],
            &["ado", "workitem", "get", "1", "--comments", "0"],
            setup,
        );
        let sent = transport.sent().into_iter();
        (
            outcome.code,
            sent.map(|s| (s.url, s.authorization)).collect(),
        )
    }

    fn vault(value: &str) -> Answer {
        Answer::json(&json!({ "value": value }))
    }

    #[test]
    fn a_pat_kv_refused_by_a_401_or_a_sign_in_page_or_redirect_is_read_again_and_sent_once_more() {
        for refusal in [
            Answer::status(401, "{}"),
            Answer::status(203, "<html>Sign in</html>"),
            Answer::status(302, "").with_header(
                "Location",
                "https://spsprodcus1.vssps.visualstudio.com/_signin",
            ),
        ] {
            let item = Answer::json(&json!({"id": 1, "rev": 1}));
            let (code, sent) = with_pat_kv(vec![vault("old-pat"), refusal, vault("new-pat"), item]);
            assert_eq!(code, 0, "{sent:?}");
            let vault_url = "https://kv-contoso.vault.azure.net/secrets/ado-pat?api-version=7.4";
            assert_eq!(
                (sent[0].0.as_str(), sent[2].0.as_str()),
                (vault_url, vault_url)
            );
            assert_eq!(sent[1].1.as_deref(), Some(basic("old-pat").expose()));
            assert_eq!(sent[3].1.as_deref(), Some(basic("new-pat").expose()));
        }
    }

    #[test]
    fn a_pat_kv_unchanged_in_key_vault_leaves_the_refusal_standing() {
        let (code, sent) = with_pat_kv(vec![
            vault("old-pat"),
            Answer::status(203, "<html>Sign in</html>"),
            vault("old-pat"),
        ]);
        assert_eq!((code, sent.len()), (3, 3), "{sent:?}");
        let (code, sent) = with_pat_kv(vec![
            vault("old-pat"),
            Answer::status(401, "{}"),
            vault("old-pat"),
            Answer::status(401, "{}"),
        ]);
        assert_eq!((code, sent.len()), (3, 4), "core's one resend: {sent:?}");
    }

    #[test]
    fn a_personal_access_token_in_the_environment_goes_as_basic() {
        let transport = FakeTransport::answering([Answer::json(&json!({"id": 1, "rev": 1}))]);
        let setup = Setup::fake(transport.clone())
            .with_config(testing::CONFIG)
            .with_env("AZURE_DEVOPS_EXT_PAT", "fixture-pat");
        let outcome = run(
            &[DOMAIN],
            &["ado", "workitem", "get", "AB#1", "--comments", "0"],
            setup,
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent()[0].authorization.as_deref(),
            Some("Basic OmZpeHR1cmUtcGF0")
        );
    }

    #[test]
    fn a_name_several_people_share_lists_each_with_their_address() {
        let (outcome, transport) = testing::ado(
            &["ado", "workitem", "list", "--assignee", "Sam Lee"],
            vec![testing::page(vec![
                json!({"id": "u-2", "providerDisplayName": "Sam Lee",
                    "properties": {"Account": {"$value": "sam@contoso.com"}}}),
                json!({"id": "u-3", "providerDisplayName": "Sam Lee",
                    "properties": {"Account": {"$value": "sam.lee@fabrikam.com"}}}),
                json!({"id": "u-4", "providerDisplayName": "Sam Leeds", "properties": {}}),
            ])],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains(
                "\"Sam Lee\" matches 2 people: Sam Lee <sam@contoso.com>, Sam Lee <sam.lee@fabrikam.com>"
            ),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1, "no query runs on a guess");
    }

    #[test]
    fn a_project_the_organization_lacks_is_exit_2_naming_the_configured_ones() {
        let (outcome, _) = testing::ado(
            &["ado", "workitem-type", "list", "--project", "Nope"],
            vec![Answer::status(
                404,
                r#"{"message":"TF200016: The following project does not exist: Nope."}"#,
            )],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: pass --project one of [ado] project: Fabrikam"),
            "{}",
            outcome.stderr
        );
    }
}
