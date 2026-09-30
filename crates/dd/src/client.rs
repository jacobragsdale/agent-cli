//! Datadog over REST: the site and its hosts, the credential, the one door
//! every request goes through, and what every command shares: the time
//! window, the tag filters, and the helpers that turn Datadog's answers into
//! rows.
//!
//! Retrying a spent token, waiting out a throttle (`X-RateLimit-Reset`),
//! refusing redirects and reading Datadog's `errors` bodies are core's. What
//! is Datadog's own is here: two ways to sign in (a bearer token, or the
//! `DD-API-KEY` and `DD-APPLICATION-KEY` pair), and one host a credential may
//! go to, `api.<site>`.

use std::sync::{Mutex, PoisonError};

use agent_cli_core::{
    Config, Credential, Ctx, Effect, Exit, Failure, Method, Op, Request, Response, Secret, Span,
    When, form_encode, host_under, status_of, utc_time,
};
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

const NO_CREDENTIAL_HINT: &str = "export DD_ACCESS_TOKEN (a personal access token), or set token_cmd = \"pup auth token\" under [datadog] to reuse a pup login; `agent-cli doctor dd` checks the setup";

/// One Datadog site: where its API and its web app live.
#[derive(Debug)]
pub(crate) struct Site {
    pub(crate) name: &'static str,
    pub(crate) app: &'static str,
    /// For the overview: `dd eu`.
    pub(crate) label: &'static str,
}

/// The sites Datadog's API spec lists. Custom hosts wait until someone needs one.
pub(crate) const SITES: &[Site] = &[
    Site {
        name: "datadoghq.com",
        app: "app.datadoghq.com",
        label: "us1",
    },
    Site {
        name: "us3.datadoghq.com",
        app: "us3.datadoghq.com",
        label: "us3",
    },
    Site {
        name: "us5.datadoghq.com",
        app: "us5.datadoghq.com",
        label: "us5",
    },
    Site {
        name: "datadoghq.eu",
        app: "app.datadoghq.eu",
        label: "eu",
    },
    Site {
        name: "ap1.datadoghq.com",
        app: "ap1.datadoghq.com",
        label: "ap1",
    },
    Site {
        name: "ap2.datadoghq.com",
        app: "ap2.datadoghq.com",
        label: "ap2",
    },
    Site {
        name: "uk1.datadoghq.com",
        app: "uk1.datadoghq.com",
        label: "uk1",
    },
    Site {
        name: "ddog-gov.com",
        app: "app.ddog-gov.com",
        label: "gov",
    },
    Site {
        name: "us2.ddog-gov.com",
        app: "us2.ddog-gov.com",
        label: "gov2",
    },
];

/// `[datadog]` (or `[dd]`) in config.toml. Credentials are named by where
/// they come from (`*_env`, `*_cmd`); a literal key in the file is refused.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Section {
    pub(crate) site: Option<String>,
    pub(crate) env: Option<String>,
    token: Option<String>,
    token_env: Option<String>,
    token_cmd: Option<String>,
    api_key: Option<String>,
    api_key_env: Option<String>,
    api_key_cmd: Option<String>,
    app_key: Option<String>,
    app_key_env: Option<String>,
    app_key_cmd: Option<String>,
}

impl Section {
    /// `[datadog]`, or `[dd]` when that is the one written.
    pub(crate) fn load(config: &Config) -> Result<Self> {
        match (config.has_section("datadog"), config.has_section("dd")) {
            (true, true) => Err(Failure::setup(format!(
                "both [datadog] and [dd] are in {}",
                config.path().display()
            ))
            .hint("keep one of them")
            .into()),
            (false, true) => config.section("dd"),
            _ => config.section("datadog"),
        }
    }
}

/// A site as written (`datadoghq.eu`, `https://app.datadoghq.eu/`), or the
/// default; one Datadog does not run is exit 3.
pub(crate) fn site(raw: Option<&str>) -> Result<&'static Site> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(&SITES[0]);
    };
    let bare = raw.to_ascii_lowercase();
    let bare = bare
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .trim_start_matches("api.");
    let bare = bare.strip_prefix("app.").unwrap_or(bare);
    SITES.iter().find(|site| site.name == bare).ok_or_else(|| {
        let names: Vec<&str> = SITES.iter().map(|site| site.name).collect();
        Failure::setup(format!("{raw:?} is not a Datadog site"))
            .hint(format!(
                "set [datadog] site to one of: {}",
                names.join(", ")
            ))
            .into()
    })
}

enum Auth {
    Token(Credential),
    Keys(Credential, Credential),
}

#[derive(Clone)]
enum Resolved {
    Bearer(Secret),
    Keys(Secret, Secret),
}

/// One Datadog site, and the credential for it.
pub(crate) struct Dd {
    pub(crate) site: &'static Site,
    /// `--env` when a command is not given one: `[datadog] env`.
    pub(crate) env: Option<String>,
    auth: Option<Auth>,
    /// Resolved on the first request, so `--dry-run` and read-only refusals
    /// never run a `token_cmd`.
    resolved: Mutex<Option<Resolved>>,
}

impl Dd {
    /// The section, the site (`[datadog] site`, then `DD_SITE`, then
    /// `datadoghq.com`) and where the credential comes from: a token first,
    /// then the key pair, then `DD_ACCESS_TOKEN`, then `DD_API_KEY` with
    /// `DD_APP_KEY`.
    pub(crate) fn load(ctx: &Ctx) -> Result<Self> {
        let section = Section::load(ctx.config())?;
        let site = site(
            section
                .site
                .clone()
                .or_else(|| ctx.env("DD_SITE"))
                .as_deref(),
        )?;
        for (key, value) in [
            ("token", &section.token),
            ("api_key", &section.api_key),
            ("app_key", &section.app_key),
        ] {
            if value.is_some() {
                return Err(Failure::setup(format!(
                    "[datadog] {key} holds the key itself, which is refused: a Datadog key opens a whole org"
                ))
                .hint(format!("name where it comes from: {key}_env = \"VAR\" or {key}_cmd = \"pass show NAME\""))
                .into());
            }
        }
        let setup = |message: String| Failure::setup(message).hint(NO_CREDENTIAL_HINT);
        let token = Credential::from_keys("token", None, section.token_env, section.token_cmd)
            .map_err(setup)?;
        let api = Credential::from_keys("api_key", None, section.api_key_env, section.api_key_cmd)
            .map_err(setup)?;
        let app = Credential::from_keys("app_key", None, section.app_key_env, section.app_key_cmd)
            .map_err(setup)?;
        let from_env = |key: &str, variable: &str| {
            Credential::from_keys(key, None, Some(variable.to_owned()), None)
                .ok()
                .flatten()
        };
        let auth = match (token, api, app) {
            (Some(token), _, _) => Some(Auth::Token(token)),
            (None, Some(api), Some(app)) => Some(Auth::Keys(api, app)),
            (None, Some(_), None) | (None, None, Some(_)) => {
                return Err(setup(
                    "[datadog] names one of api_key and app_key; the pair needs both".to_owned(),
                )
                .into());
            }
            (None, None, None) if ctx.env("DD_ACCESS_TOKEN").is_some() => {
                from_env("token", "DD_ACCESS_TOKEN").map(Auth::Token)
            }
            (None, None, None) if ctx.env("DD_API_KEY").is_some() => {
                match (
                    from_env("api_key", "DD_API_KEY"),
                    from_env("app_key", "DD_APP_KEY"),
                ) {
                    (Some(api), Some(app)) => Some(Auth::Keys(api, app)),
                    _ => None,
                }
            }
            (None, None, None) => None,
        };
        Ok(Self {
            site,
            env: section.env.filter(|env| !env.trim().is_empty()),
            auth,
            resolved: Mutex::new(None),
        })
    }

    /// Where the credential comes from, never what it is: for doctor.
    pub(crate) fn source(&self) -> Option<String> {
        match &self.auth {
            None => None,
            Some(Auth::Token(token)) => Some(token.source()),
            Some(Auth::Keys(api, app)) => Some(format!("{} and {}", api.source(), app.source())),
        }
    }

    pub(crate) fn uses_keys(&self) -> bool {
        matches!(self.auth, Some(Auth::Keys(..)))
    }

    fn resolve(&self, ctx: &Ctx, fresh: bool) -> Result<Resolved> {
        let mut held = self.resolved.lock().unwrap_or_else(PoisonError::into_inner);
        if !fresh && let Some(resolved) = held.as_ref() {
            return Ok(resolved.clone());
        }
        let resolved = match &self.auth {
            None => {
                return Err(Failure::setup("no Datadog credential is set up")
                    .hint(NO_CREDENTIAL_HINT)
                    .into());
            }
            Some(Auth::Token(token)) => Resolved::Bearer(token.resolve(ctx)?),
            Some(Auth::Keys(api, app)) => Resolved::Keys(api.resolve(ctx)?, app.resolve(ctx)?),
        };
        *held = Some(resolved.clone());
        Ok(resolved)
    }

    /// `https://api.<site><path>?<query>`: the only host a credential goes to.
    pub(crate) fn url(&self, path: &str, query: &[(&str, String)]) -> String {
        let mut url = format!("https://api.{}{path}", self.site.name);
        if !query.is_empty() {
            let pairs: Vec<(String, String)> = query
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect();
            url.push('?');
            url.push_str(&form_encode(&pairs));
        }
        url
    }

    /// Where the web app shows `path`: for `url` fields, never fetched.
    pub(crate) fn app(&self, path: &str) -> String {
        format!("https://{}{path}", self.site.app)
    }

    pub(crate) fn get(&self, ctx: &Ctx, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.send(ctx, None, Method::Get, self.url(path, query), None)?
            .json()
    }

    /// A `POST` that only reads: log, span and aggregate searches.
    pub(crate) fn search(&self, ctx: &Ctx, path: &str, body: Value) -> Result<Value> {
        self.send(ctx, None, Method::Query, self.url(path, &[]), Some(body))?
            .json()
    }

    /// A change, checked by core against `--dry-run`, read-only mode and
    /// `--yes`. An empty answer (a `204`) reads as `null`.
    pub(crate) fn change(
        &self,
        ctx: &Ctx,
        effect: Effect,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let response = self.send(ctx, Some(effect), method, self.url(path, &[]), body)?;
        if response.body.trim().is_empty() {
            return Ok(Value::Null);
        }
        response.json()
    }

    pub(crate) fn send(
        &self,
        ctx: &Ctx,
        effect: Option<Effect>,
        method: Method,
        url: String,
        body: Option<Value>,
    ) -> Result<Response> {
        let call = Call {
            dd: self,
            method,
            url,
            body,
        };
        match effect {
            None => ctx.read(call),
            Some(effect) => ctx.write(effect, call),
        }
        .map_err(forbidden)
    }

    /// An id (`4711`) or the web URL that shows it: the query parameter
    /// `<marker>_id` (`/slo?slo_id=…`), else the path segment after
    /// `marker` (`https://app.datadoghq.eu/monitors/4711`). A URL is read,
    /// never fetched; one on another site is exit 2.
    pub(crate) fn web_id(&self, raw: &str, noun: &str, marker: &str) -> Result<String> {
        let raw = raw.trim();
        let wrong = |why: String| -> anyhow::Error {
            Failure::usage(why)
                .hint(format!(
                    "pass the {noun} id, or its https://{} URL",
                    self.site.app
                ))
                .into()
        };
        let Some(rest) = raw
            .strip_prefix("https://")
            .or_else(|| raw.strip_prefix("http://"))
        else {
            if raw.is_empty() || raw.contains(['/', '?', ' ']) {
                return Err(wrong(format!("{raw:?} is not a {noun} id")));
            }
            return Ok(raw.to_owned());
        };
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = host.to_ascii_lowercase();
        if host != self.site.app && host != self.site.name {
            return Err(wrong(format!(
                "{raw} is on {host}, and the configured Datadog site is {} ({})",
                self.site.name, self.site.app
            )));
        }
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let path = path.split('#').next().unwrap_or_default();
        let segments: Vec<&str> = path.split('/').collect();
        let found = query
            .split(['&', '#'])
            .find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                (key == format!("{marker}_id"))
                    .then(|| value.to_owned())
                    .filter(|value| !value.is_empty())
            })
            .or_else(|| {
                segments
                    .iter()
                    .position(|segment| *segment == marker)
                    .and_then(|at| segments.get(at + 1))
                    .filter(|id| !id.is_empty())
                    .map(|id| (*id).to_owned())
            });
        found.ok_or_else(|| wrong(format!("{raw} is not a {noun} URL")))
    }
}

/// One request with the Datadog credential, attached when it is performed:
/// after core's read-only and `--dry-run` checks, and only to `api.<site>`.
struct Call<'a> {
    dd: &'a Dd,
    method: Method,
    url: String,
    body: Option<Value>,
}

impl Call<'_> {
    fn request<'r>(&self) -> Request<'r> {
        let request =
            Request::new(self.method, self.url.clone()).header("Accept", "application/json");
        match &self.body {
            Some(body) => request.json(body.clone()),
            None => request,
        }
    }
}

impl Op for Call<'_> {
    type Output = Response;

    /// The request, with each credential header it would carry shown as `***`.
    fn plan(&self) -> Value {
        let mut plan = self.request().plan();
        let headers: &[&str] = match &self.dd.auth {
            None => &[],
            Some(Auth::Token(_)) => &["Authorization"],
            Some(Auth::Keys(..)) => &["DD-API-KEY", "DD-APPLICATION-KEY"],
        };
        for name in headers {
            plan["headers"][*name] = json!("***");
        }
        plan
    }

    fn writes(&self) -> bool {
        !self.method.is_read()
    }

    fn perform(self, ctx: &Ctx) -> Result<Response> {
        let api = format!("api.{}", self.dd.site.name);
        if !host_under(&self.url, &api) {
            bail!(
                "refusing to send the Datadog credential to {}; it goes only to {api}",
                self.url
            );
        }
        let request = self.request();
        match self.dd.resolve(ctx, false)? {
            Resolved::Bearer(_) => {
                let mint = |fresh: bool| match self.dd.resolve(ctx, fresh)? {
                    Resolved::Bearer(token) => {
                        Ok(Secret::new(format!("Bearer {}", token.expose())))
                    }
                    Resolved::Keys(..) => bail!("the Datadog credential changed kind"),
                };
                request.auth(&mint).perform(ctx)
            }
            Resolved::Keys(api_key, app_key) => request
                .header("DD-API-KEY", api_key.expose())
                .header("DD-APPLICATION-KEY", app_key.expose())
                .perform(ctx),
        }
    }
}

/// A `403` says which scope is missing in Datadog's words; the hint says
/// where that is decided.
fn forbidden(error: anyhow::Error) -> anyhow::Error {
    if status_of(&error) != Some(403) {
        return error;
    }
    let mut failure = Failure::new(Exit::Failed, format!("{error:#}")).hint(
        "the credential lacks a scope this call needs (a read-only token cannot mute); `agent-cli doctor dd` shows which credential is used",
    );
    failure.status = Some(403);
    failure.into()
}

// ---------- the time window ----------

/// `--since`/`--until` resolved once, sent in whichever unit an API wants.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Window {
    pub(crate) since: OffsetDateTime,
    pub(crate) until: OffsetDateTime,
}

impl Window {
    /// The window asked for, or the last `default` (`15m`, `1h`) before
    /// `--until`; a note says when the default applied.
    pub(crate) fn new(
        ctx: &Ctx,
        since: Option<When>,
        until: Option<When>,
        default: &str,
    ) -> Result<Self> {
        let until = until.unwrap_or_else(When::now).0;
        let since = match since {
            Some(since) => since.0,
            None => {
                let span: Span = default.parse().map_err(anyhow::Error::msg)?;
                let since = until - span.0;
                ctx.note(format!(
                    "[since {default} before --until by default: {} to {}; widen with --since]",
                    utc_time(since),
                    utc_time(until)
                ));
                since
            }
        };
        if since >= until {
            return Err(Failure::usage(format!(
                "--since {} is not before --until {}",
                utc_time(since),
                utc_time(until)
            ))
            .into());
        }
        Ok(Self { since, until })
    }

    /// Milliseconds since the epoch, as log, span and event searches take them.
    pub(crate) fn since_ms(&self) -> String {
        millis(self.since).to_string()
    }

    pub(crate) fn until_ms(&self) -> String {
        millis(self.until).to_string()
    }

    pub(crate) fn seconds(&self) -> i64 {
        (self.until - self.since).whole_seconds()
    }
}

fn millis(instant: OffsetDateTime) -> i128 {
    instant.unix_timestamp_nanos() / 1_000_000
}

// ---------- filters ----------

/// The unified service tags and the Kubernetes tags the Datadog Agent puts on
/// pod telemetry. Plain tag filters: none defaults from a k8s scope, because
/// nothing says Datadog's cluster names match them.
#[derive(clap::Args, Debug, Default)]
pub struct Scope {
    /// Datadog service tag
    #[arg(long)]
    service: Option<String>,
    /// env tag (logs, spans: default [datadog] env)
    #[arg(long)]
    env: Option<String>,
    /// kube_cluster_name tag, e.g. prod (a filter; never defaulted)
    #[arg(long)]
    cluster: Option<String>,
    /// kube_namespace tag
    #[arg(long)]
    namespace: Option<String>,
    /// A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
    #[arg(long)]
    pod: Option<String>,
    /// A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
    #[arg(long)]
    deployment: Option<String>,
}

impl Scope {
    /// Every filter as a `key:value` tag. `[datadog] env` fills in `--env`
    /// when `default_env` says it should (not for metrics and events, which
    /// often carry no env tag). A pod or deployment id whose cluster or
    /// namespace disagrees with a flag is exit 2.
    pub(crate) fn tags(&self, dd: &Dd, default_env: bool) -> Result<Vec<String>> {
        let mut tags = Vec::new();
        if let Some(service) = &self.service {
            tags.push(format!("service:{service}"));
        }
        let env = self
            .env
            .as_ref()
            .or(dd.env.as_ref().filter(|_| default_env));
        if let Some(env) = env {
            tags.push(format!("env:{env}"));
        }
        tags.extend(kube_tags(
            self.cluster.as_deref(),
            self.namespace.as_deref(),
            self.pod.as_deref(),
            self.deployment.as_deref(),
        )?);
        Ok(tags)
    }
}

/// `kube_cluster_name`, `kube_namespace`, `pod_name` and `kube_deployment`
/// tags from the flags and the ids given to `--pod` and `--deployment`.
pub(crate) fn kube_tags(
    cluster: Option<&str>,
    namespace: Option<&str>,
    pod: Option<&str>,
    deployment: Option<&str>,
) -> Result<Vec<String>> {
    let mut cluster = cluster.map(str::to_owned);
    let mut namespace = namespace.map(str::to_owned);
    let mut named = Vec::new();
    for (flag, raw, key) in [
        ("pod", pod, "pod_name"),
        ("deployment", deployment, "kube_deployment"),
    ] {
        let Some(raw) = raw.map(str::trim) else {
            continue;
        };
        let parts: Vec<&str> = raw.split('/').collect();
        let (in_cluster, in_namespace, name) = match parts[..] {
            [name] => (None, None, name),
            [namespace, name] => (None, Some(namespace), name),
            [cluster, namespace, name] => (Some(cluster), Some(namespace), name),
            _ => (None, None, ""),
        };
        if parts.iter().any(|part| part.is_empty()) || name.is_empty() {
            return Err(Failure::usage(format!(
                "--{flag} {raw:?} is not NAME, NAMESPACE/NAME or CLUSTER/NAMESPACE/NAME"
            ))
            .into());
        }
        merge(&mut cluster, in_cluster, raw, "cluster")?;
        merge(&mut namespace, in_namespace, raw, "namespace")?;
        named.push(format!("{key}:{name}"));
    }
    let mut tags = Vec::new();
    if let Some(cluster) = cluster {
        tags.push(format!("kube_cluster_name:{cluster}"));
    }
    if let Some(namespace) = namespace {
        tags.push(format!("kube_namespace:{namespace}"));
    }
    tags.extend(named);
    Ok(tags)
}

fn merge(held: &mut Option<String>, part: Option<&str>, raw: &str, what: &str) -> Result<()> {
    match (held.as_deref(), part) {
        (Some(held), Some(part)) if held != part => Err(Failure::usage(format!(
            "{raw} is in {what} {part}, and --{what} says {held}"
        ))
        .into()),
        (None, Some(part)) => {
            *held = Some(part.to_owned());
            Ok(())
        }
        _ => Ok(()),
    }
}

/// `QUERY` ANDed with the typed filters, in Datadog's search syntax.
pub(crate) fn search_query(query: Option<&str>, terms: &[String]) -> String {
    let query = query
        .map(str::trim)
        .filter(|query| !query.is_empty() && *query != "*");
    let mut parts = Vec::new();
    if let Some(query) = query {
        parts.push(
            if terms.is_empty() || !query.contains(char::is_whitespace) {
                query.to_owned()
            } else {
                format!("({query})")
            },
        );
    }
    parts.extend(terms.iter().cloned());
    if parts.is_empty() {
        "*".to_owned()
    } else {
        parts.join(" ")
    }
}

/// `key:value`, or `key:(a OR b)` for several.
pub(crate) fn any_of(key: &str, values: &[String]) -> Option<String> {
    match values {
        [] => None,
        [one] => Some(format!("{key}:{one}")),
        many => Some(format!("{key}:({})", many.join(" OR "))),
    }
}

// ---------- rows ----------

/// The value of `key:value` among `tags` (a JSON list of strings).
pub(crate) fn tag<'a>(tags: &'a Value, key: &str) -> Option<&'a str> {
    tags.as_array()?.iter().find_map(|tag| {
        let (name, value) = tag.as_str()?.split_once(':')?;
        (name == key).then_some(value)
    })
}

/// The k8s id of the pod `tags` describe, `cluster/namespace/pod`, which
/// `agent-cli k8s pod get` takes; shorter when a tag is missing.
pub(crate) fn pod_ref(tags: &Value) -> Option<String> {
    let pod = tag(tags, "pod_name")?;
    Some(
        match (tag(tags, "kube_cluster_name"), tag(tags, "kube_namespace")) {
            (Some(cluster), Some(namespace)) => format!("{cluster}/{namespace}/{pod}"),
            (_, Some(namespace)) => format!("{namespace}/{pod}"),
            _ => pod.to_owned(),
        },
    )
}

/// `text` cut to `max` characters, saying how many more there were.
pub(crate) fn cut(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}\u{2026}(+{})", count - max)
}

/// A service's RFC 3339 stamp in UTC with milliseconds, as log and span
/// times print: ordering inside a second matters there.
pub(crate) fn utc_ms(raw: &str) -> String {
    match raw.parse::<When>() {
        Ok(When(instant)) if raw.contains('T') => {
            let whole = utc_time(instant);
            format!(
                "{}.{:03}Z",
                whole.trim_end_matches('Z'),
                instant.millisecond()
            )
        }
        _ => raw.to_owned(),
    }
}

/// Epoch seconds as a printed time.
pub(crate) fn epoch(seconds: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .map(utc_time)
}

pub(crate) fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

pub(crate) fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}

/// `rows` cut to `limit`, with a note when more came back.
pub(crate) fn limited<T>(ctx: &Ctx, mut rows: Vec<T>, limit: usize) -> Vec<T> {
    if rows.len() > limit {
        ctx.note(format!("[{limit} of {}; --limit N]", rows.len()));
        rows.truncate(limit);
    }
    rows
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_site_is_one_datadog_runs_written_any_way() {
        assert_eq!(site(None).unwrap().name, "datadoghq.com");
        for raw in [
            "datadoghq.eu",
            "https://app.datadoghq.eu/",
            "api.datadoghq.eu",
            "DATADOGHQ.EU",
        ] {
            assert_eq!(site(Some(raw)).unwrap().app, "app.datadoghq.eu", "{raw}");
        }
        assert_eq!(site(Some("us5.datadoghq.com")).unwrap().label, "us5");
        let error = site(Some("datadog.contoso.example")).unwrap_err();
        assert!(
            error.to_string().contains("is not a Datadog site"),
            "{error}"
        );
    }

    #[test]
    fn k8s_ids_become_tags_and_a_disagreeing_flag_is_refused() {
        assert_eq!(
            kube_tags(None, None, Some("prod/web/api-7d9f8c6b5-x2k4q"), None).unwrap(),
            [
                "kube_cluster_name:prod",
                "kube_namespace:web",
                "pod_name:api-7d9f8c6b5-x2k4q"
            ]
        );
        assert_eq!(
            kube_tags(Some("prod"), None, None, Some("web/api")).unwrap(),
            [
                "kube_cluster_name:prod",
                "kube_namespace:web",
                "kube_deployment:api"
            ]
        );
        assert_eq!(
            kube_tags(None, Some("web"), Some("web/x"), None).unwrap(),
            ["kube_namespace:web", "pod_name:x"]
        );
        let error = kube_tags(Some("dev"), None, Some("prod/web/x"), None).unwrap_err();
        assert_eq!(
            error.to_string(),
            "prod/web/x is in cluster prod, and --cluster says dev"
        );
        assert!(kube_tags(None, None, Some("a/b/c/d"), None).is_err());
        assert!(kube_tags(None, None, Some("web/"), None).is_err());
    }

    #[test]
    fn a_query_is_anded_with_the_filters() {
        let terms = vec!["service:api".to_owned(), "status:error".to_owned()];
        assert_eq!(search_query(None, &[]), "*");
        assert_eq!(search_query(Some("*"), &terms), "service:api status:error");
        assert_eq!(
            search_query(Some("@http.status_code:502 OR timeout"), &terms),
            "(@http.status_code:502 OR timeout) service:api status:error"
        );
        assert_eq!(
            search_query(Some("timeout"), &terms),
            "timeout service:api status:error"
        );
        assert_eq!(
            any_of("status", &["error".into(), "warn".into()]).unwrap(),
            "status:(error OR warn)"
        );
    }

    #[test]
    fn rows_carry_the_k8s_pod_id_cut_messages_and_utc_times() {
        let tags = json!([
            "env:prod",
            "kube_cluster_name:prod",
            "kube_namespace:web",
            "pod_name:api-1"
        ]);
        assert_eq!(pod_ref(&tags).as_deref(), Some("prod/web/api-1"));
        assert_eq!(
            pod_ref(&json!(["kube_namespace:web", "pod_name:api-1"])).as_deref(),
            Some("web/api-1")
        );
        assert_eq!(
            pod_ref(&json!(["pod_name:api-1"])).as_deref(),
            Some("api-1")
        );
        assert_eq!(pod_ref(&json!(["kube_cluster_name:prod"])), None);
        assert_eq!(cut("abcdef", 4), "abcd\u{2026}(+2)");
        assert_eq!(cut("abc", 4), "abc");
        assert_eq!(
            utc_ms("2026-09-29T13:47:13.905+02:00"),
            "2026-09-29T11:47:13.905Z"
        );
        assert_eq!(utc_ms("2026-09-29T11:47:13Z"), "2026-09-29T11:47:13.000Z");
        assert_eq!(
            epoch(1_790_000_000).as_deref(),
            Some("2026-09-21T14:13:20Z")
        );
    }
}
