//! Azure DevOps over REST: where the organization and projects come from, the
//! credential, the one door every request goes through, and the lookups worth
//! caching for a day (who "me" is, repository and pipeline ids).
//!
//! Ported from ticket-tui's `azure.rs`. Retrying a spent token, waiting out a
//! throttle and refusing redirects are core's; what is Azure DevOps's own is
//! here: the sign-in page it answers bad credentials with, the headers and
//! API versions it wants, and where a credential may be sent.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_cli_core::{
    Ctx, Effect, Exit, Failure, Method, Request, Response, Secret, host_under, percent_encode,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// The resource `az account get-access-token` mints Azure DevOps tokens for.
const RESOURCE: &str = "499b84ac-1321-427f-aa17-267ca6975798";
pub(crate) const API: &str = "7.1";
/// Work item comments are still behind a preview flag on every 7.x version.
pub(crate) const COMMENTS_API: &str = "7.1-preview.4";
/// Connection data refuses a plain `7.1` outright.
const CONNECTION_DATA_API: &str = "7.1-preview";
/// Approvals and policy evaluations only exist as previews.
pub(crate) const PREVIEW_API: &str = "7.1-preview.1";
/// IDs change about never; a day keeps a renamed repository from lingering.
const CACHE_TTL: Duration = Duration::from_secs(24 * 3600);
const SIGN_IN_HINT: &str = "run `az login`, or set AZURE_DEVOPS_EXT_PAT to a personal access token; `agent-cli doctor ado` checks the setup";

/// `[ado]` in config.toml.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Section {
    /// A slug (`contoso`) or a URL (`https://dev.azure.com/contoso`).
    org: Option<String>,
    /// Where the work items live.
    project: Option<String>,
    /// Where the repositories, pull requests and pipelines live, when a shop
    /// keeps its board and its code in different projects.
    code_project: Option<String>,
    /// The team (or teams) whose sprint `@current` means.
    #[serde(default, deserialize_with = "one_or_many")]
    team: Vec<String>,
}

/// `team = "Web"` and `team = ["Web", "Data"]` both read.
fn one_or_many<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    let teams = match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(one) => vec![one],
        OneOrMany::Many(many) => many,
    };
    Ok(teams
        .into_iter()
        .map(|team| team.trim().to_owned())
        .filter(|team| !team.is_empty())
        .collect())
}

/// One organization and its projects, and the credential for them.
pub(crate) struct Ado {
    pub(crate) org: String,
    pub(crate) project: String,
    pub(crate) code_project: String,
    pub(crate) teams: Vec<String>,
    /// `Basic …` made from `AZURE_DEVOPS_EXT_PAT`; `None` borrows `az`'s login.
    pat: Option<Secret>,
}

impl Ado {
    /// `[ado]` (with `AGENT_CLI_ADO_*` applied by core), falling back to the
    /// `az devops configure` defaults for the organization and project.
    pub(crate) fn load(ctx: &Ctx) -> Result<Self> {
        let section: Section = ctx.section("ado")?;
        let mut ado = Self::resolve(
            section,
            || az_defaults(az_config_path()),
            ctx.config().path(),
        )?;
        ado.pat = ctx.env("AZURE_DEVOPS_EXT_PAT").map(|pat| basic(pat.trim()));
        Ok(ado)
    }

    /// The names without a credential, for the overview's status line.
    pub(crate) fn names(config: &agent_cli_core::Config) -> Result<Self> {
        let section: Section = config.section("ado")?;
        Self::resolve(section, || az_defaults(az_config_path()), config.path())
    }

    fn resolve(
        section: Section,
        defaults: impl FnOnce() -> (Option<String>, Option<String>),
        path: &Path,
    ) -> Result<Self> {
        let blank =
            |value: Option<String>| value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let (mut org, mut project) = (blank(section.org), blank(section.project));
        if org.is_none() || project.is_none() {
            let (default_org, default_project) = defaults();
            org = org.or_else(|| blank(default_org));
            project = project.or_else(|| blank(default_project));
        }
        let missing = |key: &str, example: &str, az: &str| {
            Failure::setup(format!(
                "no Azure DevOps {key}: [ado] {key} is not set in {}",
                path.display()
            ))
            .hint(format!(
                "add `{key} = \"{example}\"` under [ado] (or set AGENT_CLI_ADO_{}), or run `az devops configure --defaults {az}`",
                key.to_uppercase()
            ))
        };
        let org = org.ok_or_else(|| {
            missing(
                "org",
                "contoso",
                "organization=https://dev.azure.com/contoso",
            )
        })?;
        let project = project.ok_or_else(|| missing("project", "Fabrikam", "project=Fabrikam"))?;
        Ok(Self {
            org: org_slug(&org),
            code_project: blank(section.code_project).unwrap_or_else(|| project.clone()),
            project,
            teams: section.team,
            pat: None,
        })
    }

    /// The `Authorization` value: the PAT as `Basic`, or an `az` token as
    /// `Bearer`, minted afresh when `fresh` says the last one was refused.
    pub(crate) fn authorization(&self, ctx: &Ctx, fresh: bool) -> Result<Secret> {
        if let Some(pat) = &self.pat {
            return Ok(pat.clone());
        }
        let token = ctx.az_token(RESOURCE, fresh)?;
        Ok(Secret::new(format!("Bearer {}", token.expose())))
    }

    pub(crate) fn uses_pat(&self) -> bool {
        self.pat.is_some()
    }

    /// A read.
    pub(crate) fn get(&self, ctx: &Ctx, url: &str) -> Result<Value> {
        self.send(ctx, Method::Get, url, Body::None, None)?.json()
    }

    /// A `POST` that only reads: WIQL, a work item batch.
    pub(crate) fn query(&self, ctx: &Ctx, url: &str, body: Value) -> Result<Value> {
        self.send(ctx, Method::Query, url, Body::Json(body), None)?
            .json()
    }

    /// A change, which core checks against `--dry-run`, read-only mode and
    /// `--yes`. Some changes answer with no body; that reads as `null`.
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
    pub(crate) fn send(
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
        }
        let response = match effect {
            None => ctx.read(request),
            Some(effect) => ctx.write(effect, request),
        }
        .map_err(signed_out)?;
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

    pub(crate) fn work_item_url(&self, id: i64) -> String {
        format!(
            "{}/{}/_workitems/edit/{id}",
            self.base(),
            segment(&self.project)
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
        let Some(rest) = raw.strip_prefix("https://") else {
            let number = raw
                .strip_prefix("AB#")
                .or_else(|| raw.strip_prefix('#'))
                .unwrap_or(raw);
            return number
                .parse::<i64>()
                .ok()
                .filter(|id| *id > 0)
                .ok_or_else(|| wrong(format!("{raw:?} is not a {} id", kind.noun())));
        };
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = host.to_ascii_lowercase();
        let (org, path) = match host.strip_suffix(".visualstudio.com") {
            Some(org) => (org.to_owned(), path),
            None if host == "dev.azure.com" => {
                let (org, path) = path.split_once('/').unwrap_or((path, ""));
                (org.to_ascii_lowercase(), path)
            }
            None => return Err(wrong(format!("{raw} is not an Azure DevOps URL"))),
        };
        if org != self.org.to_ascii_lowercase() {
            return Err(wrong(format!(
                "{raw} is in organization {org}, and [ado] org is {}",
                self.org
            )));
        }
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let segments: Vec<&str> = path.split('/').collect();
        let after = |marker: &str| {
            segments
                .iter()
                .position(|segment| segment.eq_ignore_ascii_case(marker))
                .and_then(|at| segments.get(at + 1))
                .and_then(|id| id.parse::<i64>().ok())
        };
        let found = match kind {
            Kind::WorkItem => after("edit").filter(|_| path.contains("/_workitems/")),
            Kind::PullRequest => after("pullrequest"),
            Kind::Run => query
                .split('&')
                .find_map(|pair| pair.strip_prefix("buildId="))
                .and_then(|id| id.parse().ok()),
        };
        found.ok_or_else(|| wrong(format!("{raw} is not a {} URL", kind.noun())))
    }

    fn cache_key(&self, what: &str) -> String {
        format!("ado:{}:{what}", self.org.to_ascii_lowercase())
    }

    /// Who the credential signs in as. Votes and auto-complete are written
    /// under this id, and `@me` assigns to its sign-in address.
    // ponytail: cached per organization, not per credential; after signing in
    // as someone else, --no-cache (or a day) catches up.
    pub(crate) fn me(&self, ctx: &Ctx) -> Result<Me> {
        match ctx.cache().get(&self.cache_key("me")) {
            Some(me) => Ok(me),
            None => self.fetch_me(ctx),
        }
    }

    /// [`Self::me`] asked of Azure DevOps now, which is also what doctor's
    /// connection check is.
    pub(crate) fn fetch_me(&self, ctx: &Ctx) -> Result<Me> {
        let data = self.get(
            ctx,
            &self.api(None, "connectionData", "", CONNECTION_DATA_API),
        )?;
        let user = &data["authenticatedUser"];
        let me = Me {
            id: user["id"]
                .as_str()
                .context("Azure DevOps did not say who is signed in")?
                .to_owned(),
            name: text(&user["providerDisplayName"])
                .or_else(|| text(&user["customDisplayName"]))
                .unwrap_or_default(),
            account: text(&user["properties"]["Account"]["$value"]),
        };
        ctx.cache().put(&self.cache_key("me"), &me, CACHE_TTL);
        Ok(me)
    }

    /// The identity id a name, an address or `@me` stands for, which is what
    /// the pull request search filters by.
    pub(crate) fn identity(&self, ctx: &Ctx, who: &str) -> Result<String> {
        if who.eq_ignore_ascii_case("@me") {
            return Ok(self.me(ctx)?.id);
        }
        let key = self.cache_key(&format!("identity:{}", who.to_lowercase()));
        if let Some(id) = ctx.cache().get(&key) {
            return Ok(id);
        }
        let url = format!(
            "https://vssps.dev.azure.com/{}/_apis/identities?searchFilter=General&filterValue={}&queryMembership=None&api-version={API}",
            segment(&self.org),
            query_value(who)
        );
        let found = self.get(ctx, &url)?;
        // (id, display name, whether a name or address is exactly `who`)
        let people: Vec<(String, String, bool)> = list(&found["value"])
            .iter()
            .filter_map(|identity| {
                let names = [
                    &identity["providerDisplayName"],
                    &identity["properties"]["Mail"]["$value"],
                    &identity["properties"]["Account"]["$value"],
                ];
                let exact = names.iter().any(|name| {
                    name.as_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case(who))
                });
                Some((
                    identity["id"].as_str()?.to_owned(),
                    text(names[0]).unwrap_or_default(),
                    exact,
                ))
            })
            .collect();
        let exact: Vec<&(String, String, bool)> = people.iter().filter(|person| person.2).collect();
        let chosen = match (exact.as_slice(), people.as_slice()) {
            ([one], _) => *one,
            (_, [one]) => one,
            (_, []) => {
                return Err(
                    Failure::not_found(format!("nobody in {} matches {who:?}", self.org))
                        .hint("give a full sign-in address, such as jane@contoso.com")
                        .into(),
                );
            }
            (_, many) => {
                let names: Vec<&str> = many
                    .iter()
                    .take(5)
                    .map(|person| person.1.as_str())
                    .collect();
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

    /// One repository by name or id; a miss in the cache asks again before it
    /// says there is no such repository, since the cache can predate it.
    pub(crate) fn repo(&self, ctx: &Ctx, name: &str) -> Result<RepoRef> {
        let find = |repos: Vec<RepoRef>| {
            repos.into_iter().find(|repo| {
                repo.name.eq_ignore_ascii_case(name) || repo.id.eq_ignore_ascii_case(name)
            })
        };
        if let Some(repo) = find(self.repos(ctx, false)?) {
            return Ok(repo);
        }
        find(self.repos(ctx, true)?).ok_or_else(|| {
            Failure::not_found(format!(
                "there is no repository {name:?} in {}",
                self.code_project
            ))
            .hint("agent-cli ado repo list --fields name")
            .into()
        })
    }

    /// A pipeline's id: as given when it is a number, else looked up by name.
    pub(crate) fn pipeline_id(&self, ctx: &Ctx, name: &str) -> Result<i64> {
        if let Ok(id) = name.trim().parse() {
            return Ok(id);
        }
        let key = self.cache_key(&format!(
            "pipeline:{}:{}",
            self.code_project.to_lowercase(),
            name.to_lowercase()
        ));
        if let Some(id) = ctx.cache().get(&key) {
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
                    "agent-cli ado pipeline list {name} --fields id,name"
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
        ctx.cache().put(&key, &id, CACHE_TTL);
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
pub(crate) enum Body {
    None,
    Json(Value),
    /// A JSON Patch document, which Azure DevOps only takes under its own
    /// media type.
    Patch(Value),
}

/// The signed-in user, as connection data describes them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Me {
    pub(crate) id: String,
    pub(crate) name: String,
    /// The sign-in address, which is what an assignment is written with.
    pub(crate) account: Option<String>,
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

/// Where an Azure DevOps credential may go: the organization's hosts and the
/// identity host, never a URL an answer handed back that points elsewhere.
fn trusted(url: &str) -> bool {
    host_under(url, "dev.azure.com") || host_under(url, ".visualstudio.com")
}

/// Core reads a refused credential as needs-setup; this names the two ways to
/// fix it for Azure DevOps. An organization backed by a Microsoft account
/// refuses with a redirect to its sign-in page instead of a 401, which is the
/// same answer.
fn signed_out(error: anyhow::Error) -> anyhow::Error {
    match error.downcast::<Failure>() {
        Ok(failure)
            if failure.exit == Exit::Setup || failure.message.contains("which is not followed") =>
        {
            Failure::setup(failure.message).hint(SIGN_IN_HINT).into()
        }
        Ok(failure) => failure.into(),
        Err(error) => error,
    }
}

/// `Basic` credentials from a personal access token: `AZURE_DEVOPS_EXT_PAT`,
/// the variable the Azure DevOps CLI extension reads too.
fn basic(pat: &str) -> Secret {
    // Made a Secret so redaction also masks the bare value.
    let _ = Secret::new(pat);
    Secret::new(format!("Basic {}", base64(format!(":{pat}").as_bytes())))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut buffer = [0_u8; 3];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let bits = u32::from(buffer[0]) << 16 | u32::from(buffer[1]) << 8 | u32::from(buffer[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                let value = (bits >> (18 - 6 * index)) & 0x3f;
                output.push(char::from(TABLE[value as usize]));
            } else {
                output.push('=');
            }
        }
    }
    output
}

/// `https://dev.azure.com/contoso`, `https://contoso.visualstudio.com` or `contoso`.
fn org_slug(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if let Some(rest) = trimmed.strip_prefix("https://dev.azure.com/") {
        return rest.split('/').next().unwrap_or(rest).to_owned();
    }
    if let Some(slug) = trimmed
        .strip_prefix("https://")
        .and_then(|rest| rest.strip_suffix(".visualstudio.com"))
    {
        return slug.to_owned();
    }
    trimmed.to_owned()
}

/// The organization and project `az devops configure --defaults` saved.
pub(crate) fn az_defaults(path: Option<PathBuf>) -> (Option<String>, Option<String>) {
    let Some(raw) = path.and_then(|path| std::fs::read_to_string(path).ok()) else {
        return (None, None);
    };
    let (mut org, mut project) = (None, None);
    for line in raw.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "organization" => org = Some(value.trim().to_owned()),
            "project" => project = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    (org, project)
}

pub(crate) fn az_config_path() -> Option<PathBuf> {
    let dir = std::env::var_os("AZURE_CONFIG_DIR").map_or_else(
        || std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".azure")),
        |dir| Some(PathBuf::from(dir)),
    )?;
    Some(dir.join("azuredevops").join("config"))
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
    use crate::{DOMAIN, testing};
    use agent_cli_core::testing::run;
    use agent_cli_core::testing::{Answer, FakeTransport, ctx};
    use agent_cli_core::{Config, Setup};
    use serde_json::json;

    use super::*;

    fn section(toml: &str) -> Section {
        Config::parse("c.toml", Some(toml), Vec::new())
            .section("ado")
            .unwrap()
    }

    #[test]
    fn config_takes_urls_or_slugs_one_team_or_several_and_the_code_project_defaults() {
        let none = || (None, None);
        let ado = Ado::resolve(
            section("[ado]\norg = \"https://dev.azure.com/contoso/\"\nproject = \"Fabrikam\"\nteam = \"Web Team\"\n"),
            none,
            Path::new("c.toml"),
        )
        .unwrap();
        assert_eq!(
            (ado.org.as_str(), ado.project.as_str()),
            ("contoso", "Fabrikam")
        );
        assert_eq!(ado.code_project, "Fabrikam");
        assert_eq!(ado.teams, ["Web Team"]);

        let ado = Ado::resolve(
            section("[ado]\norg = \"https://contoso.visualstudio.com\"\nproject = \"Board\"\ncode_project = \"Code\"\nteam = [\"A\", \" \", \"B\"]\n"),
            none,
            Path::new("c.toml"),
        )
        .unwrap();
        assert_eq!(
            (ado.org.as_str(), ado.code_project.as_str()),
            ("contoso", "Code")
        );
        assert_eq!(ado.teams, ["A", "B"]);
    }

    #[test]
    fn a_missing_org_or_project_falls_back_to_az_defaults_then_is_needs_setup() {
        let defaults = || {
            (
                Some("https://dev.azure.com/fabrikam".to_owned()),
                Some("Web".to_owned()),
            )
        };
        let ado = Ado::resolve(Section::default(), defaults, Path::new("c.toml")).unwrap();
        assert_eq!(
            (ado.org.as_str(), ado.project.as_str()),
            ("fabrikam", "Web")
        );

        let error = Ado::resolve(
            section("[ado]\norg = \"contoso\"\n"),
            || (None, None),
            Path::new("/x/c.toml"),
        )
        .err()
        .unwrap();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.exit, Exit::Setup);
        assert_eq!(
            failure.message,
            "no Azure DevOps project: [ado] project is not set in /x/c.toml"
        );
        assert!(
            failure
                .hint
                .as_deref()
                .unwrap()
                .contains("AGENT_CLI_ADO_PROJECT")
        );

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        std::fs::write(
            &file,
            "[defaults]\norganization = https://dev.azure.com/contoso/\nproject = Fabrikam\n",
        )
        .unwrap();
        assert_eq!(
            az_defaults(Some(file)),
            (
                Some("https://dev.azure.com/contoso/".into()),
                Some("Fabrikam".into())
            )
        );
        assert_eq!(az_defaults(Some(dir.path().join("missing"))), (None, None));
    }

    #[test]
    fn a_pat_goes_as_basic_and_segments_escape_spaces_and_plus() {
        assert_eq!(basic("abc").expose(), "Basic OmFiYw==");
        assert_eq!(base64(b"any carnal pleas"), "YW55IGNhcm5hbCBwbGVhcw==");
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
            pat: Some(basic("fixture-pat")),
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
            pat: Some(basic("fixture-pat")),
            ..Ado::resolve(
                section("[ado]\norg=\"contoso\"\nproject=\"Fabrikam\"\n"),
                || (None, None),
                Path::new("c"),
            )
            .unwrap()
        };
        assert_eq!(
            ado.me(&ctx).unwrap().account.as_deref(),
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
}
