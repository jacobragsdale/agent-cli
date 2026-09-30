//! `acr`: container registry images. Ported from az-tui's `azure::acr` and its
//! `repos` / `tags` commands, with the TUI's attribute fill and manifest view.
//!
//! ARM does not sign these calls. A registry mints its own tokens: the `az`
//! token for the containerregistry audience is traded for a refresh token
//! (`/oauth2/exchange`), and that for an access token scoped to the one thing
//! about to be read (`/oauth2/token`). Both are form posts that only read, and
//! neither is sent anywhere but an `https://` host under `.azurecr.io`. The
//! `/acr/v1/` endpoints are used because they carry attributes; `/v2/` carries
//! names only.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use agent_cli_core::{
    Check, Config, Ctx, Failure, Method, Request, Secret, When, command, host_under, percent_encode,
};
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::graph::{Registry, inventory};
use crate::{Azure, Kind, REGISTRY, az_token, limited, narrow, parallel, stamp, text};

const CATALOG_SCOPE: &str = "registry:catalog:*";
/// A short page is how a listing's end announces itself.
const PAGE: usize = 100;

const REGISTRIES: Kind = Kind {
    noun: "registry",
    key: "registries",
    flag: "--registry",
    domain: "acr",
};

// ---------- the token chain ----------

/// One registry's tokens for one command: a refresh token, and an access
/// token per scope, each minted once and again after a 401.
struct Session<'a> {
    ctx: &'a Ctx,
    registry: &'a Registry,
    refresh: Mutex<Option<Secret>>,
    access: Mutex<HashMap<String, Secret>>,
}

impl<'a> Session<'a> {
    /// The login server comes from Resource Graph (or the cache) and is about
    /// to be handed an `az` token, so it has to be a registry's own host.
    fn new(ctx: &'a Ctx, registry: &'a Registry) -> Result<Self> {
        if !host_under(
            &format!("https://{}/", registry.login_server),
            ".azurecr.io",
        ) {
            bail!(
                "{}: {:?} is not a registry login server; no token is sent there",
                registry.name,
                registry.login_server
            );
        }
        Ok(Self {
            ctx,
            registry,
            refresh: Mutex::new(None),
            access: Mutex::new(HashMap::new()),
        })
    }

    /// One signed read of `/acr/v1/{path}`.
    fn get(&self, scope: &str, path: &str) -> Result<Value> {
        let url = format!("https://{}/acr/v1/{path}", self.registry.login_server);
        let mint = |fresh: bool| self.bearer(scope, fresh);
        self.ctx
            .read(Request::get(url).auth(&mint))
            .map_err(|error| self.explain(error))?
            .json()
    }

    /// `Bearer <access token>` for `scope`. After a 401 (`fresh`) the whole
    /// chain is minted again, down to the `az` token, or a spent one would be
    /// traded for another spent one.
    fn bearer(&self, scope: &str, fresh: bool) -> Result<Secret> {
        if fresh {
            *lock(&self.refresh) = None;
            lock(&self.access).remove(scope);
        }
        let held = lock(&self.access).get(scope).cloned();
        let access = match held {
            Some(held) => held,
            None => {
                let refresh = self.refresh_token(fresh)?;
                let issued = self.post(
                    "token",
                    vec![
                        ("grant_type", "refresh_token".to_owned()),
                        ("service", self.registry.login_server.clone()),
                        ("scope", scope.to_owned()),
                        ("refresh_token", refresh.expose().to_owned()),
                    ],
                )?;
                let access = Secret::new(text(&issued["access_token"]).with_context(|| {
                    format!(
                        "{} answered the token call without a token",
                        self.registry.login_server
                    )
                })?);
                lock(&self.access).insert(scope.to_owned(), access.clone());
                access
            }
        };
        Ok(Secret::new(format!("Bearer {}", access.expose())))
    }

    /// The registry's refresh token, traded for the `az` token once per run.
    /// A 401 on the trade is most often a stale `az` token: one fresh one,
    /// one retry.
    fn refresh_token(&self, fresh: bool) -> Result<Secret> {
        if let Some(held) = lock(&self.refresh).clone() {
            return Ok(held);
        }
        let exchanged = match self.exchange(fresh) {
            Err(error) if !fresh && crate::refused_with(&error) == Some(401) => self.exchange(true),
            first => first,
        }
        .map_err(|error| self.explain(error))?;
        let refresh = Secret::new(text(&exchanged["refresh_token"]).with_context(|| {
            format!(
                "{} answered the exchange without a token",
                self.registry.login_server
            )
        })?);
        *lock(&self.refresh) = Some(refresh.clone());
        Ok(refresh)
    }

    // ponytail: no `tenant` field. The swagger calls it optional and `az acr`
    // sends it; add it (from `az account show`) if a registry refuses without.
    fn exchange(&self, fresh: bool) -> Result<Value> {
        let cli = az_token(self.ctx, REGISTRY, fresh)?;
        self.post(
            "exchange",
            vec![
                ("grant_type", "access_token".to_owned()),
                ("service", self.registry.login_server.clone()),
                ("access_token", cli.expose().to_owned()),
            ],
        )
    }

    /// One of the two token posts: form fields in, JSON out, no bearer.
    fn post(&self, endpoint: &str, fields: Vec<(&str, String)>) -> Result<Value> {
        let url = format!("https://{}/oauth2/{endpoint}", self.registry.login_server);
        let fields = fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        self.ctx
            .read(Request::new(Method::Query, url).form(fields))?
            .json()
    }

    /// A 401 or 403 that survived the retry is a login with no role here, which
    /// no retry fixes. Anything else, `az` not signed in included, keeps its
    /// own words.
    fn explain(&self, error: anyhow::Error) -> anyhow::Error {
        if matches!(crate::refused_with(&error), Some(401 | 403)) {
            return error.context(format!(
                "{}: no permission (needs AcrPull, or Container Registry Repository Reader on an ABAC registry)",
                self.registry.login_server
            ));
        }
        error
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---------- the data plane ----------

/// A repository name in a path: `team/api` keeps its slash.
fn path(repo: &str) -> String {
    let mut encoded = String::new();
    for segment in repo.split('/') {
        if !encoded.is_empty() {
            encoded.push('/');
        }
        percent_encode(segment, &mut encoded);
    }
    encoded
}

fn metadata(repo: &str) -> String {
    format!("repository:{repo}:metadata_read")
}

/// Every repository in one registry, by name.
fn catalog(session: &Session<'_>) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    let mut last: Option<String> = None;
    loop {
        let mut query = format!("_catalog?n={PAGE}");
        if let Some(last) = &last {
            query.push_str("&last=");
            percent_encode(last, &mut query);
        }
        let page = session.get(CATALOG_SCOPE, &query)?;
        let listed: Vec<String> = page["repositories"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(text)
            .collect();
        let full = listed.len() >= PAGE;
        let previous = last.clone();
        last = listed.last().cloned();
        names.extend(listed);
        // A page that did not move the cursor would be asked for for ever.
        if !full || last.is_none() || last == previous {
            return Ok(names);
        }
    }
}

/// A repository's counts and stamps, which the catalog does not carry. Cached
/// per registry for `[azure] refresh` seconds.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Attributes {
    tag_count: Option<u64>,
    manifest_count: Option<u64>,
    created: Option<String>,
    updated: Option<String>,
}

fn attributes(session: &Session<'_>, repo: &str) -> Result<Attributes> {
    let answer = session.get(&metadata(repo), &path(repo))?;
    Ok(Attributes {
        tag_count: count(&answer["tagCount"]),
        manifest_count: count(&answer["manifestCount"]),
        created: stamp(&answer["createdTime"]),
        updated: stamp(&answer["lastUpdateTime"]),
    })
}

/// A registry sends counts as numbers, but a string of digits is the same.
fn count(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

// ---------- acr registry list ----------

#[derive(clap::Args)]
pub struct RegistryListArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn registry_list(ctx: &Ctx, args: RegistryListArgs) -> Result<Vec<Registry>> {
    let azure = Azure::load(ctx)?;
    let found = narrow(
        &inventory(ctx, &azure)?.registries,
        &azure.registries,
        &[],
        REGISTRIES,
        |r| &r.name,
    )?;
    Ok(limited(ctx, found, args.limit))
}

command! {
    pub REGISTRY_LIST = ["acr", "registry", "list"], Read,
    "List the container registries the az login reaches (within [azure] registries)",
    keywords: ["acr", "login", "server", "inventory", "docker"],
    example: "acr registry list --fields name,login_server",
    run: registry_list,
}

// ---------- acr repo list ----------

#[derive(clap::Args)]
pub struct RepoListArgs {
    /// Part of the repository name, any case
    name: Option<String>,
    /// Only this registry (repeatable; within [azure] registries)
    #[arg(long)]
    registry: Vec<String>,
    /// Only repositories pushed to after this
    #[arg(long)]
    since: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RepoRow {
    /// `loginserver/repository`: what `acr tag list` takes.
    id: String,
    registry: String,
    repository: String,
    tag_count: Option<u64>,
    manifest_count: Option<u64>,
    /// RFC 3339.
    created: Option<String>,
    /// When something was last pushed to it.
    updated: Option<String>,
}

fn repo_list(ctx: &Ctx, args: RepoListArgs) -> Result<Vec<RepoRow>> {
    let within = args.since;
    let azure = Azure::load(ctx)?;
    let registries = narrow(
        &inventory(ctx, &azure)?.registries,
        &azure.registries,
        &args.registry,
        REGISTRIES,
        |r| &r.name,
    )?;
    let sessions = registries
        .iter()
        .map(|registry| Session::new(ctx, registry))
        .collect::<Result<Vec<_>>>()?;
    let wanted = args.name.as_deref().map(str::to_ascii_lowercase);
    let mut names: Vec<(usize, String)> = Vec::new();
    let mut failed = Vec::new();
    for (index, listed) in parallel(&sessions, azure.parallel(), catalog)
        .into_iter()
        .enumerate()
    {
        match listed {
            Ok(listed) => names.extend(
                listed
                    .into_iter()
                    .filter(|name| {
                        wanted
                            .as_deref()
                            .is_none_or(|w| name.to_ascii_lowercase().contains(w))
                    })
                    .map(|name| (index, name)),
            ),
            Err(error) => failed.push(error),
        }
    }
    if !failed.is_empty() && failed.len() == sessions.len() {
        return Err(failed.remove(0));
    }
    for error in failed {
        ctx.note(format!("[not read: {error:#}]"));
    }
    let total = names.len();
    // Without a time filter only the rows that will print need attributes.
    if within.is_none() {
        names.truncate(args.limit);
    }
    let filled = fill(ctx, &azure, &sessions, &names)?;
    let rows: Vec<RepoRow> = names
        .into_iter()
        .zip(filled)
        .filter(|(_, held)| {
            within.is_none_or(|since| {
                held.updated
                    .as_deref()
                    .and_then(crate::parse_stamp)
                    .is_some_and(|at| at >= since.0)
            })
        })
        .map(|((index, repository), held)| RepoRow {
            id: format!("{}/{repository}", registries[index].login_server),
            registry: registries[index].name.clone(),
            repository,
            tag_count: held.tag_count,
            manifest_count: held.manifest_count,
            created: held.created,
            updated: held.updated,
        })
        .collect();
    if within.is_none() && total > rows.len() {
        ctx.note(format!("[{} of {total}; --limit N]", rows.len()));
        return Ok(rows);
    }
    Ok(limited(ctx, rows, args.limit))
}

/// The attributes of each `(registry, repository)`, from the cache where it
/// has them and read side by side where it does not. A repository gone since
/// the catalog, or one this login may list but not read (ABAC), keeps its
/// name with the rest left empty; any other failure is the answer.
fn fill(
    ctx: &Ctx,
    azure: &Azure,
    sessions: &[Session<'_>],
    names: &[(usize, String)],
) -> Result<Vec<Attributes>> {
    let key = |index: usize| format!("acr:attributes:{}", sessions[index].registry.login_server);
    let mut cached: Vec<BTreeMap<String, Attributes>> = (0..sessions.len())
        .map(|index| ctx.cache().get(&key(index)).unwrap_or_default())
        .collect();
    let missing: Vec<&(usize, String)> = names
        .iter()
        .filter(|(index, name)| !cached[*index].contains_key(name))
        .collect();
    let read = parallel(&missing, azure.parallel(), |(index, name)| {
        attributes(&sessions[*index], name)
    });
    let mut skipped = 0;
    let mut touched = vec![false; sessions.len()];
    for ((index, name), result) in missing.into_iter().zip(read) {
        match result {
            Ok(held) => {
                cached[*index].insert(name.clone(), held);
                touched[*index] = true;
            }
            Err(error) if matches!(crate::refused_with(&error), Some(403 | 404)) => skipped += 1,
            Err(error) => return Err(error),
        }
    }
    for (index, map) in cached.iter().enumerate() {
        if touched[index] {
            ctx.cache().put(&key(index), map, azure.refresh());
        }
    }
    if skipped > 0 {
        ctx.note(format!("[{skipped} repositories' counts and dates did not load (gone, or not readable by this login)]"));
    }
    Ok(names
        .iter()
        .map(|(index, name)| cached[*index].get(name).cloned().unwrap_or_default())
        .collect())
}

command! {
    pub REPO_LIST = ["acr", "repo", "list"], Read,
    "List container image repositories with tag counts and last push",
    keywords: ["image", "images", "repository", "repositories", "pushed", "recent", "docker"],
    example: "acr repo list api --since 7d --fields id,tag_count,updated",
    run: repo_list,
}

// ---------- acr tag list ----------

#[derive(clap::Args)]
pub struct TagListArgs {
    /// The repository: its exact name (team/api) or id (contosoacr.azurecr.io/team/api)
    repo: String,
    /// The registry that holds it; needed when more than one does
    #[arg(long)]
    registry: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TagRow {
    /// The image, `loginserver/repository:tag`: what `acr manifest get` takes.
    id: String,
    tag: String,
    digest: String,
    /// RFC 3339.
    created: Option<String>,
    updated: Option<String>,
}

fn tag_list(ctx: &Ctx, args: TagListArgs) -> Result<Vec<TagRow>> {
    let azure = Azure::load(ctx)?;
    let image = image_ref(&args.repo);
    if image.reference.is_some() {
        return Err(Failure::usage(format!(
            "{} names one image; tag list takes its repository",
            args.repo
        ))
        .hint(format!("agent-cli acr manifest get {}", args.repo))
        .into());
    }
    let registry = registry_for(ctx, &azure, &image, args.registry.as_deref())?;
    let args = TagListArgs {
        repo: image.repo,
        ..args
    };
    let session = Session::new(ctx, &registry)?;
    let mut tags: Vec<TagRow> = Vec::new();
    let mut last: Option<String> = None;
    // `orderby=timedesc` makes it newest first; paging stops one past the
    // limit, which is enough to say there are more.
    while tags.len() <= args.limit {
        let mut query = format!("{}/_tags?n={PAGE}&orderby=timedesc", path(&args.repo));
        if let Some(last) = &last {
            query.push_str("&last=");
            percent_encode(last, &mut query);
        }
        let page = session.get(&metadata(&args.repo), &query)?;
        let listed: Vec<TagRow> = page["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let tag = text(&entry["name"])?;
                Some(TagRow {
                    id: format!("{}/{}:{tag}", registry.login_server, args.repo),
                    tag,
                    digest: text(&entry["digest"]).unwrap_or_default(),
                    created: stamp(&entry["createdTime"]),
                    updated: stamp(&entry["lastUpdateTime"]),
                })
            })
            .collect();
        let full = listed.len() >= PAGE;
        let previous = last.clone();
        last = listed.last().map(|tag| tag.tag.clone());
        tags.extend(listed);
        if !full || last.is_none() || last == previous {
            break;
        }
    }
    if tags.len() > args.limit {
        ctx.note(format!(
            "[first {} tags, newest first; --limit N for more]",
            args.limit
        ));
        tags.truncate(args.limit);
    }
    Ok(tags)
}

/// An image as an agent was handed it: `team/api`, with `:tag` or
/// `@sha256:…`, after the login server (`contosoacr.azurecr.io/team/api:1.42.0`)
/// or not. Read, never fetched.
struct ImageRef {
    host: Option<String>,
    repo: String,
    reference: Option<String>,
}

fn image_ref(raw: &str) -> ImageRef {
    let raw = raw.trim();
    let (host, rest) = match raw.split_once('/') {
        Some((first, rest)) if first.contains(['.', ':']) || first == "localhost" => {
            (Some(first.to_ascii_lowercase()), rest)
        }
        _ => (None, raw),
    };
    let (repo, reference) = match rest.split_once('@') {
        Some((repo, digest)) => (repo, Some(digest)),
        None => match rest.rsplit_once(':') {
            Some((repo, tag)) if !tag.contains('/') => (repo, Some(tag)),
            _ => (rest, None),
        },
    };
    ImageRef {
        host,
        repo: repo.to_owned(),
        reference: reference.map(str::to_owned),
    }
}

/// The registry that holds `repo`: the one its login server names (which
/// must be in reach, and agree with `--registry`), else as [`holder`] finds it.
fn registry_for(
    ctx: &Ctx,
    azure: &Azure,
    image: &ImageRef,
    flag: Option<&str>,
) -> Result<Registry> {
    let Some(host) = &image.host else {
        return holder(ctx, azure, &image.repo, flag);
    };
    let found = crate::allowed(&inventory(ctx, azure)?.registries, &azure.registries, |r| {
        &r.name
    });
    let Some(registry) = found
        .iter()
        .find(|registry| registry.login_server.eq_ignore_ascii_case(host))
    else {
        let servers: Vec<&str> = found.iter().map(|r| r.login_server.as_str()).collect();
        return Err(Failure::usage(format!(
            "{host} is not a registry in reach; they are: {}",
            if servers.is_empty() {
                "none".to_owned()
            } else {
                servers.join(", ")
            }
        ))
        .into());
    };
    match flag {
        Some(flag) if !flag.eq_ignore_ascii_case(&registry.name) => Err(Failure::usage(format!(
            "{host} is registry {}, and --registry says {flag}",
            registry.name
        ))
        .into()),
        _ => Ok(registry.clone()),
    }
}

/// The one registry holding `repo`: `--registry`, or the only one in reach,
/// or the one whose catalog names it. Two is exit 2, never a guess.
fn holder(ctx: &Ctx, azure: &Azure, repo: &str, only: Option<&str>) -> Result<Registry> {
    let only: Vec<String> = only.map(str::to_owned).into_iter().collect();
    let registries = narrow(
        &inventory(ctx, azure)?.registries,
        &azure.registries,
        &only,
        REGISTRIES,
        |r| &r.name,
    )?;
    match registries.as_slice() {
        [] => {
            return Err(Failure::setup("the login reaches no container registries")
                .hint("az login, or check the subscriptions with `agent-cli doctor acr`")
                .into());
        }
        [one] => return Ok(one.clone()),
        _ => {}
    }
    let sessions = registries
        .iter()
        .map(|registry| Session::new(ctx, registry))
        .collect::<Result<Vec<_>>>()?;
    let mut holding: Vec<Registry> = Vec::new();
    let mut failed = Vec::new();
    for (session, listed) in sessions
        .iter()
        .zip(parallel(&sessions, azure.parallel(), catalog))
    {
        match listed {
            Ok(names) if names.iter().any(|name| name == repo) => {
                holding.push(session.registry.clone())
            }
            Ok(_) => {}
            Err(error) => failed.push(error),
        }
    }
    if holding.is_empty()
        && let Some(error) = failed.into_iter().next()
    {
        return Err(error);
    }
    let names = |found: &[Registry]| {
        found
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match holding.len() {
        1 => Ok(holding.remove(0)),
        0 => Err(
            Failure::not_found(format!("no repository {repo} in {}", names(&registries)))
                .hint(format!(
                    "agent-cli acr repo list {repo} --fields registry,repository"
                ))
                .into(),
        ),
        _ => Err(Failure::usage(format!(
            "{repo} is in more than one registry ({}); name one with --registry",
            names(&holding)
        ))
        .into()),
    }
}

command! {
    pub TAG_LIST = ["acr", "tag", "list"], Read,
    "List an image repository's tags, newest first, with their digests",
    keywords: ["tags", "image", "images", "version", "versions", "latest", "pushed", "docker"],
    example: "acr tag list team/api --fields id,digest,updated",
    run: tag_list,
}

// ---------- acr manifest get ----------

#[derive(clap::Args)]
pub struct ManifestGetArgs {
    /// The image: contosoacr.azurecr.io/team/api:1.42.0, team/api@sha256:…, or a repository
    image: String,
    /// A tag (1.42.0) or a digest (sha256:…), when IMAGE names only the repository
    reference: Option<String>,
    /// The registry that holds it; needed when more than one does
    #[arg(long)]
    registry: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ManifestRow {
    digest: String,
    /// Bytes, as the registry counts them.
    size: Option<u64>,
    /// None for a multi-arch index.
    architecture: Option<String>,
    os: Option<String>,
    /// RFC 3339.
    created: Option<String>,
    /// Every tag pointing at it.
    tags: Vec<String>,
    /// What `docker pull` takes, pinned to the digest.
    pull: String,
}

fn manifest_get(ctx: &Ctx, args: ManifestGetArgs) -> Result<ManifestRow> {
    let azure = Azure::load(ctx)?;
    let image = image_ref(&args.image);
    let reference = match (image.reference.clone(), args.reference) {
        (Some(held), Some(given)) if held != given => {
            return Err(Failure::usage(format!(
                "{} names {held}, and REFERENCE says {given}",
                args.image
            ))
            .into());
        }
        (Some(held), _) => held,
        (None, Some(given)) => given,
        (None, None) => {
            return Err(
                Failure::usage(format!("{} names no tag or digest", args.image))
                    .hint(format!(
                        "agent-cli acr tag list {} --fields id,updated",
                        args.image
                    ))
                    .into(),
            );
        }
    };
    let registry = registry_for(ctx, &azure, &image, args.registry.as_deref())?;
    let repo = image.repo;
    let session = Session::new(ctx, &registry)?;
    let mut encoded = String::new();
    percent_encode(&reference, &mut encoded);
    let reference = encoded;
    // A digest's colon survives as itself; the registry reads either form.
    let reference = reference.replace("%3A", ":");
    let answer = session.get(
        &metadata(&repo),
        &format!("{}/_manifests/{reference}", path(&repo)),
    )?;
    // The singular call nests everything under `manifest`.
    let held = if answer["manifest"].is_object() {
        &answer["manifest"]
    } else {
        &answer
    };
    let digest = text(&held["digest"]).unwrap_or(reference);
    Ok(ManifestRow {
        pull: format!("{}/{}@{digest}", registry.login_server, repo),
        size: count(&held["imageSize"]),
        architecture: text(&held["architecture"]),
        os: text(&held["os"]),
        created: stamp(&held["createdTime"]),
        tags: held["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(text)
            .collect(),
        digest,
    })
}

command! {
    pub MANIFEST_GET = ["acr", "manifest", "get"], Read,
    "Describe one image by tag or digest: size, architecture, os, tags on it",
    keywords: ["image", "digest", "sha256", "pull", "reference", "arch", "platform", "size"],
    example: "acr manifest get contosoacr.azurecr.io/team/api:1.42.0 --fields digest,created,tags,pull",
    run: manifest_get,
}

// ---------- overview and doctor ----------

pub fn status(config: &Config) -> String {
    crate::allowlist_status(config, "acr", ("registry", "registries"), |azure| {
        &azure.registries
    })
}

/// The login and the registry token, the inventory, and the first catalog
/// page of each registry: the exchange is where a missing role shows.
pub fn doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("azure") {
        return Vec::new();
    }
    let mut checks = Vec::new();
    let Some(azure) = crate::doctor_login(ctx, &mut checks, &[("registry token", REGISTRY)]) else {
        return checks;
    };
    let inventory = match inventory(ctx, &azure) {
        Ok(inventory) => inventory,
        Err(error) => {
            checks.push(Check::failed(
                "inventory",
                format!("{error:#}"),
                "az account list; check [azure] subscriptions",
            ));
            return checks;
        }
    };
    let registries = crate::allowed(&inventory.registries, &azure.registries, |r| &r.name);
    checks.push(Check::ok(
        "inventory",
        format!("{} registries in reach", registries.len()),
    ));
    for name in crate::missing(&inventory.registries, &azure.registries, |r| &r.name) {
        checks.push(Check::failed(
            format!("registry {name}"),
            "not found by Resource Graph in the subscriptions in scope",
            "fix [azure] registries or subscriptions; `agent-cli acr registry list` shows what is there",
        ));
    }
    for registry in &registries {
        let check = format!("registry {}", registry.name);
        if ctx.remaining().is_err() {
            checks.push(Check::failed(
                check,
                "not checked: --timeout ran out",
                "agent-cli doctor acr --timeout 120",
            ));
            continue;
        }
        let started = std::time::Instant::now();
        let probe = Session::new(ctx, registry)
            .and_then(|session| session.get(CATALOG_SCOPE, "_catalog?n=1"));
        checks.push(match probe {
            Ok(_) => Check::ok(check, format!("{} answered in {} ms", registry.login_server, started.elapsed().as_millis())),
            Err(error) => Check::failed(check, format!("{error:#}"), "needs AcrPull (or Container Registry Repository Catalog Lister on an ABAC registry)"),
        });
    }
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, Sent};
    use serde_json::json;

    use super::*;
    use crate::ACR;
    use crate::fixtures::{self, azure};

    fn exchanged() -> Answer {
        Answer::json(&json!({"refresh_token": "refresh-fixture-1"}))
    }

    fn issued(token: &str) -> Answer {
        Answer::json(&json!({"access_token": token}))
    }

    fn one_registry() -> Answer {
        fixtures::inventory(vec![fixtures::registry("contosoacr")])
    }

    fn field<'a>(sent: &'a Sent, key: &str) -> Option<&'a str> {
        sent.body.as_ref()?[key].as_str()
    }

    #[test]
    fn repo_list_fills_counts_and_dates_the_catalog_leaves_out() {
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "repo", "list"],
            vec![
                one_registry(),
                exchanged(),
                issued("catalog-token"),
                Answer::json(&json!({"repositories": ["team/api", "team/web"]})),
                issued("api-token"),
                Answer::json(
                    &json!({"imageName": "team/api", "tagCount": "48", "manifestCount": 51,
                    "createdTime": "2025-09-11T18:00:00Z", "lastUpdateTime": "2026-09-11T18:00:00Z"}),
                ),
                issued("web-token"),
                Answer::json(&json!({"imageName": "team/web", "tagCount": 3})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["repository"], "team/api");
        assert_eq!(
            rows[0]["tag_count"], 48,
            "a count written as digits is the same count"
        );
        assert_eq!(rows[0]["manifest_count"], 51);
        assert_eq!(rows[0]["updated"], "2026-09-11T18:00:00Z");
        assert_eq!(rows[1]["tag_count"], 3);
        let sent = transport.sent();
        assert_eq!(
            sent.iter()
                .filter(|s| s.url.ends_with("/oauth2/exchange"))
                .count(),
            1,
            "one exchange for every scope"
        );
        assert_eq!(sent[1].url, "https://contosoacr.azurecr.io/oauth2/exchange");
        assert_eq!(field(&sent[1], "grant_type"), Some("access_token"));
        assert_eq!(
            field(&sent[1], "access_token"),
            Some("token@https://containerregistry.azure.net"),
            "the containerregistry audience, not ARM"
        );
        assert!(
            sent[1].authorization.is_none(),
            "a token post carries no bearer"
        );
        assert_eq!(field(&sent[2], "scope"), Some("registry:catalog:*"));
        assert_eq!(field(&sent[2], "refresh_token"), Some("refresh-fixture-1"));
        assert_eq!(
            sent[3].url,
            "https://contosoacr.azurecr.io/acr/v1/_catalog?n=100"
        );
        assert_eq!(
            sent[3].authorization.as_deref(),
            Some("Bearer catalog-token")
        );
        assert_eq!(
            field(&sent[4], "scope"),
            Some("repository:team/api:metadata_read")
        );
        assert_eq!(sent[5].url, "https://contosoacr.azurecr.io/acr/v1/team/api");
    }

    #[test]
    fn the_catalog_pages_with_last_and_a_short_page_ends_it() {
        let full: Vec<String> = (0..PAGE).map(|n| format!("repo-{n:03}")).collect();
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "repo", "list", "last", "--limit", "0"],
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(&json!({"repositories": full})),
                Answer::json(&json!({"repositories": ["repo-last"]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome.stderr.contains("[0 of 1; --limit N]"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent()[4].url,
            "https://contosoacr.azurecr.io/acr/v1/_catalog?n=100&last=repo-099"
        );
    }

    #[test]
    fn a_spent_data_plane_token_redoes_the_whole_chain_once() {
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "tag", "list", "team/api"],
            vec![
                one_registry(),
                exchanged(),
                issued("stale"),
                Answer::status(
                    401,
                    r#"{"errors":[{"code":"UNAUTHORIZED","message":"expired"}]}"#,
                ),
                exchanged(),
                issued("fresh"),
                Answer::json(&json!({"tags": [{"name": "1.0", "digest": "sha256:ab"}]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert_eq!(
            field(&sent[4], "access_token"),
            Some("token-fresh@https://containerregistry.azure.net"),
            "down to a fresh az token"
        );
        assert_eq!(sent[6].authorization.as_deref(), Some("Bearer fresh"));
    }

    #[test]
    fn a_refused_exchange_names_the_missing_role_after_one_retry() {
        let refused = || {
            Answer::status(
                401,
                r#"{"errors":[{"code":"UNAUTHORIZED","message":"authentication required"}]}"#,
            )
        };
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "tag", "list", "team/api"],
            vec![one_registry(), refused(), refused()],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("contosoacr.azurecr.io: no permission (needs AcrPull"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent().len(),
            3,
            "one fresh az token, then no more"
        );
        assert_eq!(
            field(&transport.sent()[2], "access_token"),
            Some("token-fresh@https://containerregistry.azure.net")
        );
    }

    #[test]
    fn no_token_goes_to_a_login_server_off_azurecr_io() {
        let mut bad = fixtures::registry("contosoacr");
        bad["loginServer"] = json!("contosoacr.azurecr.io.evil.example");
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "tag", "list", "team/api"],
            vec![fixtures::inventory(vec![bad])],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("is not a registry login server"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1, "only the inventory went out");
    }

    #[test]
    fn a_repository_in_two_registries_is_ambiguous_and_in_none_is_not_found() {
        let two = || {
            fixtures::inventory(vec![
                fixtures::registry("contosoacr"),
                fixtures::registry("fabrikamacr"),
            ])
        };
        let catalog = |names: &[&str]| Answer::json(&json!({"repositories": names}));
        let (outcome, _) = azure(
            &[ACR],
            &["acr", "tag", "list", "team/api"],
            vec![
                two(),
                exchanged(),
                issued("a"),
                catalog(&["team/api"]),
                exchanged(),
                issued("b"),
                catalog(&["team/api"]),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("team/api is in more than one registry (contosoacr, fabrikamacr)"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = azure(
            &[ACR],
            &["acr", "tag", "list", "team/api"],
            vec![
                two(),
                exchanged(),
                issued("a"),
                catalog(&["x"]),
                exchanged(),
                issued("b"),
                catalog(&[]),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }

    #[test]
    fn tags_keep_the_registrys_newest_first_order_and_stop_at_the_limit() {
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "tag", "list", "team/api", "--limit", "1"],
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(&json!({"tags": [
                    {"name": "1.42.0", "digest": "sha256:ab12", "createdTime": "2026-09-11T18:00:00Z", "lastUpdateTime": "2026-09-11T18:00:00Z"},
                    {"name": "1.41.3", "digest": "sha256:9f01", "createdTime": "2026-09-08T18:00:00Z"},
                ]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "contosoacr.azurecr.io/team/api:1.42.0", "tag": "1.42.0", "digest": "sha256:ab12",
                "created": "2026-09-11T18:00:00Z", "updated": "2026-09-11T18:00:00Z"}])
        );
        assert!(
            outcome.stderr.contains("[first 1 tags"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent()[3].url,
            "https://contosoacr.azurecr.io/acr/v1/team/api/_tags?n=100&orderby=timedesc"
        );
    }

    #[test]
    fn a_manifest_by_tag_reads_from_under_its_own_key_with_a_pinned_pull_reference() {
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "manifest", "get", "team/api", "1.42.0"],
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(
                    &json!({"registry": "contosoacr.azurecr.io", "imageName": "team/api", "manifest": {
                        "digest": "sha256:ab12ef0199", "imageSize": 84_200_000_u64, "createdTime": "2026-09-11T18:00:00Z",
                        "architecture": "amd64", "os": "linux", "tags": ["1.42.0", "1.42"],
                    }}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let manifest = outcome.json();
        assert_eq!(manifest["size"], 84_200_000);
        assert_eq!(manifest["tags"], json!(["1.42.0", "1.42"]));
        assert_eq!(
            manifest["pull"],
            "contosoacr.azurecr.io/team/api@sha256:ab12ef0199"
        );
        assert_eq!(
            transport.sent()[3].url,
            "https://contosoacr.azurecr.io/acr/v1/team/api/_manifests/1.42.0"
        );

        let (_, transport) = azure(
            &[ACR],
            &["acr", "manifest", "get", "team/api", "sha256:ff"],
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(&json!({"manifest": {"digest": "sha256:ff"}})),
            ],
        );
        assert_eq!(
            transport.sent()[3].url,
            "https://contosoacr.azurecr.io/acr/v1/team/api/_manifests/sha256:ff"
        );
    }

    #[test]
    fn an_image_reference_names_its_registry_repository_and_tag_or_digest() {
        let manifest = || Answer::json(&json!({"manifest": {"digest": "sha256:9f2c"}}));
        for (image, url) in [
            (
                "contosoacr.azurecr.io/team/api:1.42.0",
                "https://contosoacr.azurecr.io/acr/v1/team/api/_manifests/1.42.0",
            ),
            (
                "CONTOSOACR.azurecr.io/team/api@sha256:9f2c",
                "https://contosoacr.azurecr.io/acr/v1/team/api/_manifests/sha256:9f2c",
            ),
            (
                "team/api:1.42.0",
                "https://contosoacr.azurecr.io/acr/v1/team/api/_manifests/1.42.0",
            ),
        ] {
            let (outcome, transport) = azure(
                &[ACR],
                &["acr", "manifest", "get", image],
                vec![one_registry(), exchanged(), issued("t"), manifest()],
            );
            assert_eq!(outcome.code, 0, "{image}: {outcome:?}");
            assert_eq!(transport.sent()[3].url, url, "{image}");
        }
        let (outcome, _) = azure(
            &[ACR],
            &[
                "acr",
                "manifest",
                "get",
                "fabrikamacr.azurecr.io/team/api:1",
            ],
            vec![one_registry()],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains(
                "fabrikamacr.azurecr.io is not a registry in reach; they are: contosoacr.azurecr.io"
            ),
            "{}",
            outcome.stderr
        );
        let (outcome, transport) = azure(&[ACR], &["acr", "manifest", "get", "team/api"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());

        let (outcome, transport) = azure(
            &[ACR],
            &[
                "acr",
                "tag",
                "list",
                "contosoacr.azurecr.io/team/api",
                "--fields",
                "id",
            ],
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(&json!({"tags": [{"name": "1.42.0", "digest": "sha256:ab12"}]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "contosoacr.azurecr.io/team/api:1.42.0"}])
        );
        assert_eq!(
            transport.sent()[3].url,
            "https://contosoacr.azurecr.io/acr/v1/team/api/_tags?n=100&orderby=timedesc"
        );
    }

    #[test]
    fn attributes_are_cached_per_registry_and_a_gone_repository_keeps_its_name() {
        let temp = tempfile::tempdir().unwrap();
        let run = |argv: &[&str], answers: Vec<Answer>| {
            let transport = agent_cli_core::testing::FakeTransport::answering(answers);
            let mut setup =
                agent_cli_core::Setup::fake(transport.clone()).with_config(fixtures::SERIAL);
            setup.cache_dir = Some(temp.path().to_owned());
            (agent_cli_core::testing::run(&[ACR], argv, setup), transport)
        };
        let (outcome, _) = run(
            &["acr", "repo", "list"],
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(&json!({"repositories": ["gone", "team/api"]})),
                issued("t1"),
                Answer::status(
                    404,
                    r#"{"errors":[{"code":"NAME_UNKNOWN","message":"repository name not known"}]}"#,
                ),
                issued("t2"),
                Answer::json(&json!({"tagCount": 2, "lastUpdateTime": "2026-09-11T18:00:00Z"})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[0],
            json!({"id": "contosoacr.azurecr.io/gone", "registry": "contosoacr", "repository": "gone"})
        );
        assert!(
            outcome.stderr.contains("1 repositories' counts"),
            "{}",
            outcome.stderr
        );

        let (outcome, transport) = run(
            &["acr", "repo", "list", "api"],
            vec![
                exchanged(),
                issued("t"),
                Answer::json(&json!({"repositories": ["gone", "team/api"]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["tag_count"], 2);
        assert_eq!(
            transport.sent().len(),
            3,
            "the inventory and the attributes came from the cache"
        );

        for (since, want) in [
            (
                "2026-09-01",
                json!([{"id": "contosoacr.azurecr.io/team/api"}]),
            ),
            ("2026-09-12T00:00:00Z", json!([])),
        ] {
            let (outcome, _) = run(
                &["acr", "repo", "list", "--since", since, "--fields", "id"],
                vec![
                    exchanged(),
                    issued("t"),
                    Answer::json(&json!({"repositories": ["gone", "team/api"]})),
                    issued("t1"),
                    Answer::status(
                        404,
                        r#"{"errors":[{"code":"NAME_UNKNOWN","message":"gone"}]}"#,
                    ),
                ],
            );
            assert_eq!(outcome.code, 0, "{outcome:?}");
            assert_eq!(outcome.json(), want, "pushed since {since}");
        }
    }

    #[test]
    fn doctor_reads_one_catalog_page_per_registry() {
        let (ctx, transport) = fixtures::doctor_ctx(
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(&json!({"repositories": ["team/api"]})),
            ],
            "[azure]\n",
        );
        let checks = doctor(&ctx);
        assert_eq!(
            fixtures::rows(&checks),
            [
                ("az login".to_owned(), true),
                ("registry token".to_owned(), true),
                ("inventory".to_owned(), true),
                ("registry contosoacr".to_owned(), true),
            ]
        );
        assert_eq!(
            transport.sent()[3].url,
            "https://contosoacr.azurecr.io/acr/v1/_catalog?n=1"
        );
    }
}
