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

pub(crate) mod manifest;
pub(crate) mod registry;
pub(crate) mod repo;
pub(crate) mod tag;

use std::collections::HashMap;
use std::sync::Mutex;

use agent_cli_core::{Ctx, Failure, Method, Request, Secret, host_under, percent_encode};
use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::client::{Kind, REGISTRY, az_token, narrow, parallel, text};
use crate::config::Azure;
use crate::graph::{Registry, inventory};

pub(crate) const CATALOG_SCOPE: &str = "registry:catalog:*";
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
pub(crate) struct Session<'a> {
    ctx: &'a Ctx,
    registry: &'a Registry,
    refresh: Mutex<Option<Secret>>,
    access: Mutex<HashMap<String, Secret>>,
}

impl<'a> Session<'a> {
    /// The login server comes from Resource Graph (or the cache) and is about
    /// to be handed an `az` token, so it has to be a registry's own host.
    pub(crate) fn new(ctx: &'a Ctx, registry: &'a Registry) -> Result<Self> {
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
    pub(crate) fn get(&self, scope: &str, path: &str) -> Result<Value> {
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
            Err(error) if !fresh && crate::client::refused_with(&error) == Some(401) => {
                self.exchange(true)
            }
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
        if matches!(crate::client::refused_with(&error), Some(401 | 403)) {
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

/// A registry sends counts as numbers, but a string of digits is the same.
fn count(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
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
    let found =
        crate::client::allowed(&inventory(ctx, azure)?.registries, &azure.registries, |r| {
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

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::ACR;
    use crate::testing::{self, azure, exchanged, field, issued, one_registry};

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
        let mut bad = testing::registry("contosoacr");
        bad["loginServer"] = json!("contosoacr.azurecr.io.evil.example");
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "tag", "list", "team/api"],
            vec![testing::inventory(vec![bad])],
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
}
