//! Airflow over its REST API, Airflow 3's `/api/v2` or 2's `/api/v1`: the
//! `[airflow]` section and the one door every request goes through. `auth`
//! says which API a server speaks and signs in; `id` holds the ids every
//! command takes.
//!
//! Core's `host_under` does not fit a configured server (it wants https with
//! no port, and a compose Airflow is `http://localhost:8080`), but every URL
//! this crate sends is built from `base_url`: paging is by offset, the log
//! token is a query value, and Airflow never hands back a URL to follow. So
//! the door attaches the token only to URLs that start with `base_url/`
//! exactly, and `base_url` itself is checked when the section loads.

use std::cell::{OnceCell, RefCell};

use agent_cli_core::{
    Config, Credential, Ctx, Effect, Exit, Failure, Method, Request, Response, Secret,
    percent_encode, pick, status_of, utc,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

mod auth;
mod id;

use auth::REMEMBER;
use id::disagree;
pub(crate) use id::{Ref, Want, ti_id};

use crate::instance::check_base_url;
use crate::refused::refused;

/// Airflow caps a page at `[api] maximum_page_limit`, 100 unless raised.
const PAGE: usize = 100;
/// Where the Helm chart's docs install Airflow, and so where its
/// KubernetesExecutor starts task pods unless `k8s_namespace` says otherwise.
const CHART_NAMESPACE: &str = "airflow";
/// Which REST API a server speaks.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Api {
    /// Airflow 2 (2.9 and later 2.x).
    V1,
    /// Airflow 3.
    V2,
}

/// `[airflow]` in config.toml.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    instance: Vec<Raw>,
}

/// One `[[airflow.instance]]` as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    name: String,
    base_url: String,
    api: Option<Api>,
    username: Option<String>,
    password: Option<String>,
    password_env: Option<String>,
    password_cmd: Option<String>,
    token: Option<String>,
    token_env: Option<String>,
    token_cmd: Option<String>,
    #[serde(default)]
    read_only: bool,
    k8s_scope: Option<String>,
    k8s_namespace: Option<String>,
    dags_repo: Option<String>,
}

pub(crate) enum Auth {
    /// Airflow 3: a JWT from `POST {base_url}/auth/token` (the Simple, FAB and
    /// Keycloak managers). Airflow 2: HTTP Basic, else the sign-in form.
    Password {
        username: String,
        password: Credential,
    },
    /// A bearer token as it is (Astro, Composer, MWAA).
    Token(Credential),
}

/// One Airflow server, checked.
pub(crate) struct Instance {
    pub(crate) name: String,
    /// No trailing slash, no `/api/v2`.
    pub(crate) base_url: String,
    /// The API `api` names; otherwise asked of the server.
    pub(crate) api: Option<Api>,
    pub(crate) auth: Auth,
    pub(crate) read_only: bool,
    /// The `[[k8s.scope]]` its KubernetesExecutor pods run in.
    pub(crate) k8s_scope: Option<String>,
    pub(crate) k8s_namespace: String,
    /// `REPO[:FOLDER]` in Azure DevOps that its DAG files deploy from.
    pub(crate) dags_repo: Option<String>,
}

impl Instance {
    /// `auth` for `instance list` and doctor: the kind and where it comes
    /// from, never the value.
    pub(crate) fn auth_source(&self) -> String {
        match &self.auth {
            Auth::Password { username, password } => {
                format!("password for {username} ({})", password.source())
            }
            Auth::Token(token) => format!("token ({})", token.source()),
        }
    }

    /// The k8s id of the pod a task instance ran in: `scope/namespace/pod`,
    /// what `k8s pod logs` and `k8s event list --pod` take. KubernetesExecutor
    /// names the task instance's `hostname` after its pod (a CeleryExecutor
    /// worker may give an FQDN, so only the part before the first dot).
    pub(crate) fn pod(&self, hostname: Option<&str>) -> Option<String> {
        let scope = self.k8s_scope.as_deref()?;
        let pod = hostname?.split('.').next().filter(|pod| !pod.is_empty())?;
        Some(format!("{scope}/{}/{pod}", self.k8s_namespace))
    }
}

/// Which instance a command talks to; every command but `instance list`
/// takes it.
#[derive(Clone, Debug, clap::Args)]
pub struct At {
    /// The [[airflow.instance]] name; defaults to the only one
    #[arg(long)]
    pub(crate) instance: Option<String>,
}

/// Every configured instance.
pub(crate) struct Airflow {
    pub(crate) instances: Vec<Instance>,
}

impl Airflow {
    /// The `[airflow]` section; exit 3 naming the file when it is wrong.
    pub(crate) fn load(config: &Config) -> Result<Self> {
        let section: Section = config.section("airflow")?;
        let path = config.path().display().to_string();
        let mut instances: Vec<Instance> = Vec::new();
        for raw in section.instance {
            let name = raw.name.clone();
            let problem = if instances.iter().any(|earlier| earlier.name == name) {
                Err("is named twice".to_owned())
            } else {
                raw.check()
            };
            let instance = problem.map_err(|why| {
                Failure::setup(format!("[[airflow.instance]] {name:?} in {path}: {why}"))
                    .hint("fix it; `agent-cli config example airflow` shows every key")
            })?;
            instances.push(instance);
        }
        Ok(Self { instances })
    }

    /// The instance `--instance` names, or the only one.
    pub(crate) fn instance(&self, wanted: Option<&str>) -> Result<&Instance> {
        pick(
            "Airflow instance",
            "--instance",
            wanted,
            &self.instances,
            |instance| &instance.name,
        )
        .map_err(|failure| match failure.exit {
            Exit::Usage => failure.hint("agent-cli airflow instance list").into(),
            _ => failure
                .hint("add an [[airflow.instance]]; `agent-cli config example airflow` shows the keys")
                .into(),
        })
    }

    /// A client for the instance `--instance` names.
    pub(crate) fn open<'a>(&'a self, ctx: &'a Ctx, wanted: Option<&str>) -> Result<Client<'a>> {
        Ok(Client::new(ctx, self.instance(wanted)?))
    }

    /// A client and the thing `raw` names, `DAG/latest` resolved: see
    /// [`Self::find`].
    pub(crate) fn locate<'a>(
        &'a self,
        ctx: &'a Ctx,
        wanted: Option<&str>,
        raw: &str,
        want: Want,
        dag: Option<&str>,
        run: Option<&str>,
    ) -> Result<(Client<'a>, Ref)> {
        let (instance, mut id) = self.find(wanted, raw, want, dag, run)?;
        let client = Client::new(ctx, instance);
        client.resolve(&mut id)?;
        Ok((client, id))
    }

    /// The instance and the thing `raw` names: an id (`DAG`, `DAG/RUN`,
    /// `DAG/RUN/TASK[:MAP][/TRY]`), its leading pieces given as `--dag` and
    /// `--run` instead, or an Airflow UI URL under a configured `base_url`
    /// (read, never fetched), which also picks the instance. A ref and a
    /// flag that disagree is exit 2.
    pub(crate) fn find(
        &self,
        wanted: Option<&str>,
        raw: &str,
        want: Want,
        dag: Option<&str>,
        run: Option<&str>,
    ) -> Result<(&Instance, Ref)> {
        let raw = raw.trim();
        if !(raw.starts_with("https://") || raw.starts_with("http://")) {
            let id = Ref::parse(raw, want, dag, run).map_err(|why| {
                Failure::usage(why).hint(format!("ids look like {}", want.shape()))
            })?;
            return Ok((self.instance(wanted)?, id));
        }
        let (instance, id) = self.parse_url(raw, want)?;
        if let Some(wanted) = wanted.filter(|wanted| *wanted != instance.name) {
            return Err(Failure::usage(format!(
                "{raw} is on instance {}, and --instance says {wanted}",
                instance.name
            ))
            .into());
        }
        for (held, flag, what) in [(&id.dag, dag, "dag"), (&id.run, run, "run")] {
            disagree(raw, held, flag, what).map_err(Failure::usage)?;
        }
        Ok((instance, id))
    }

    fn parse_url(&self, raw: &str, want: Want) -> Result<(&Instance, Ref)> {
        let url = raw.split('#').next().unwrap_or_default();
        let (url, query) = url.split_once('?').unwrap_or((url, ""));
        let Some(instance) = self
            .instances
            .iter()
            .find(|instance| same_origin(&instance.base_url, url))
        else {
            let configured: Vec<String> = self
                .instances
                .iter()
                .map(|instance| format!("{} ({})", instance.name, instance.base_url))
                .collect();
            return Err(Failure::usage(format!(
                "{raw} is not under a configured base_url; configured: {}",
                if configured.is_empty() {
                    "none".to_owned()
                } else {
                    configured.join(", ")
                }
            ))
            .hint("pass the id instead, or add the server as an [[airflow.instance]]")
            .into());
        };
        let rest = &url[instance.base_url.len() + 1..];
        let parts: Vec<String> = rest
            .split('/')
            .filter(|part| !part.is_empty())
            .map(decode)
            .collect();
        let after = |marker: &str| {
            parts
                .iter()
                .position(|part| part == marker)
                .and_then(|at| parts.get(at + 1))
                .cloned()
        };
        let mut id = Ref {
            dag: after("dags").unwrap_or_default(),
            run: after("runs").unwrap_or_default(),
            task: after("tasks").unwrap_or_default(),
            map: after("mapped").and_then(|map| map.parse().ok()),
            attempt: query
                .split('&')
                .find_map(|pair| pair.strip_prefix("try_number="))
                .and_then(|attempt| attempt.parse().ok()),
        };
        // A deeper URL than the command needs names its DAG or run too.
        if want != Want::Task {
            id.task.clear();
        }
        if want == Want::Dag {
            id.run.clear();
        }
        let missing = match want {
            Want::Dag => id.dag.is_empty(),
            Want::Run => id.dag.is_empty() || id.run.is_empty(),
            Want::Task => id.dag.is_empty() || id.run.is_empty() || id.task.is_empty(),
        };
        if missing {
            return Err(Failure::usage(format!(
                "{raw} does not name a {}; it needs {}",
                want.noun(),
                want.url_shape()
            ))
            .into());
        }
        Ok((instance, id))
    }
}

impl Raw {
    fn check(self) -> Result<Instance, String> {
        if self.name.trim().is_empty() {
            return Err("name is empty; --instance picks an instance by it".to_owned());
        }
        let base_url = check_base_url(&self.base_url)?;
        let password = Credential::from_keys(
            "password",
            self.password,
            self.password_env,
            self.password_cmd,
        )?;
        let token = Credential::from_keys("token", self.token, self.token_env, self.token_cmd)?;
        let auth = match (self.username, password, token) {
            (Some(username), Some(password), None) => Auth::Password { username, password },
            (None, None, Some(token)) => Auth::Token(token),
            (Some(_), None, None) => {
                return Err("username needs password_env, password_cmd or password".to_owned());
            }
            (None, Some(_), None) => return Err("a password needs username".to_owned()),
            (_, Some(_), Some(_)) => {
                return Err("give a username and password, or a token, not both".to_owned());
            }
            (Some(_), None, Some(_)) => {
                return Err("a token needs no username; drop it".to_owned());
            }
            (None, None, None) => {
                return Err(
                    "no credential: give username with password_env (or password_cmd), or token_env (or token_cmd)"
                        .to_owned(),
                );
            }
        };
        let k8s_scope = self
            .k8s_scope
            .map(|scope| scope.trim().to_owned())
            .filter(|scope| !scope.is_empty());
        Ok(Instance {
            name: self.name,
            base_url,
            api: self.api,
            auth,
            read_only: self.read_only,
            k8s_scope,
            k8s_namespace: self
                .k8s_namespace
                .map(|namespace| namespace.trim().to_owned())
                .filter(|namespace| !namespace.is_empty())
                .unwrap_or_else(|| CHART_NAMESPACE.to_owned()),
            dags_repo: self.dags_repo.filter(|repo| !repo.trim().is_empty()),
        })
    }
}

// ponytail: `url` under `base` and a `/`, the token check `host_under` (https, no ports) can't
// make for a compose Airflow; move it to core when a second domain has a configurable base URL.
pub(crate) fn same_origin(base: &str, url: &str) -> bool {
    url.strip_prefix(base)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// A URL path segment with every `%XX` decoded.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A path segment: everything outside RFC 3986's unreserved set escaped, so
/// a run id's `:` and `+` arrive as written.
pub(crate) fn segment(raw: &str) -> String {
    let mut out = String::new();
    percent_encode(raw, &mut out);
    out.replace('+', "%20")
}

/// A query value.
pub(crate) fn query_value(raw: &str) -> String {
    let mut out = String::new();
    percent_encode(raw, &mut out);
    out
}

/// One instance, its API, and the credential for it, minted at most once
/// per command.
pub(crate) struct Client<'a> {
    pub(crate) ctx: &'a Ctx,
    pub(crate) instance: &'a Instance,
    /// The `Authorization` value. JWTs stay in memory, never on disk.
    token: RefCell<Option<Secret>>,
    api: OnceCell<Api>,
    /// Airflow 2's session cookie, once its sign-in form gave one.
    session: RefCell<Option<Secret>>,
}

impl<'a> Client<'a> {
    pub(crate) fn new(ctx: &'a Ctx, instance: &'a Instance) -> Self {
        Self {
            ctx,
            instance,
            token: RefCell::new(None),
            api: OnceCell::new(),
            session: RefCell::new(None),
        }
    }

    /// Exit 2 on an instance marked `read_only`, before anything is sent;
    /// every change checks it first, `--dry-run` included.
    pub(crate) fn writable(&self) -> Result<()> {
        if self.instance.read_only {
            return Err(Failure::usage(format!(
                "instance {:?} is read_only, so this change was refused",
                self.instance.name
            ))
            .hint("read here, or make the change on an instance without read_only = true")
            .into());
        }
        Ok(())
    }

    /// True on Airflow 2, whose `/api/v1` names some paths, parameters and
    /// fields differently and lacks a few.
    pub(crate) fn v1(&self) -> Result<bool> {
        Ok(self.api()? == Api::V1)
    }

    pub(crate) fn url(&self, path: &str) -> Result<String> {
        let api = match self.api()? {
            Api::V1 => "v1",
            Api::V2 => "v2",
        };
        Ok(format!("{}/api/{api}/{path}", self.instance.base_url))
    }

    pub(crate) fn get(&self, path: &str) -> Result<Value> {
        self.send(Method::Get, path, None, None, "application/json")?
            .json()
    }

    /// A read as plain text: Airflow 2's task logs, whose JSON form is a
    /// Python list's repr.
    pub(crate) fn text(&self, path: &str) -> Result<String> {
        Ok(self.send(Method::Get, path, None, None, "text/plain")?.body)
    }

    /// A read that needs no credential (version, health), so doctor can tell
    /// a wrong `base_url` from a wrong password.
    pub(crate) fn public(&self, path: &str) -> Result<Value> {
        let request = Request::get(self.url(path)?).header("Accept", "application/json");
        self.ctx
            .read(request)
            .map_err(|error| refused(error, path))?
            .json()
    }

    /// The scheduler's, triggerer's and database's health: `monitor/health`
    /// on Airflow 3, `health` on 2.
    pub(crate) fn health(&self) -> Result<Value> {
        self.public(if self.v1()? {
            "health"
        } else {
            "monitor/health"
        })
    }

    /// A POST that only reads: a clear's server-side dry run.
    pub(crate) fn preview(&self, path: &str, body: Value) -> Result<Value> {
        self.send(Method::Query, path, Some(body), None, "application/json")?
            .json()
    }

    /// A change, which core checks against `--dry-run`, read-only mode and
    /// `--yes`.
    pub(crate) fn change(
        &self,
        effect: Effect,
        method: Method,
        path: &str,
        body: Value,
    ) -> Result<Value> {
        self.send(method, path, Some(body), Some(effect), "application/json")?
            .json()
    }

    /// How a password signed in, for doctor.
    pub(crate) fn signed_in_by(&self) -> Option<&'static str> {
        match (&self.instance.auth, self.v1().unwrap_or(false)) {
            (Auth::Token(_), _) => None,
            (Auth::Password { .. }, false) => Some("a JWT from /auth/token"),
            (Auth::Password { .. }, true) if self.session.borrow().is_some() => {
                Some("the sign-in form (the API takes only session auth)")
            }
            (Auth::Password { .. }, true) => Some("HTTP Basic"),
        }
    }

    /// The one door: the credential only under `base_url`, and Airflow's
    /// refusals read as the next step to take. Airflow 2 takes a password
    /// as HTTP Basic when `[api] auth_backends` lists `basic_auth` (the
    /// official compose does); its default lists only `session`, so a
    /// refused Basic signs in through the web form once and sends its cookie.
    fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        effect: Option<Effect>,
        accept: &str,
    ) -> Result<Response> {
        let url = self.url(path)?;
        if !same_origin(&self.instance.base_url, &url) {
            bail!("refusing to send the Airflow credential to {url}");
        }
        let form = self.v1()? && matches!(self.instance.auth, Auth::Password { .. });
        let remembered = self.remembered("session");
        if form
            && self.session.borrow().is_none()
            && self.ctx.cache().get::<bool>(&remembered).is_some()
        {
            self.sign_in_form()?;
        }
        let mint = |fresh: bool| self.authorization(fresh);
        let basic = |_: bool| self.basic();
        let attempt = || {
            let mut request = Request::new(method, &url).header("Accept", accept);
            let cookie = self.session.borrow().clone();
            request = match (cookie, form) {
                // Masked in a --dry-run plan, as every cookie header is.
                (Some(cookie), _) => request.header("Cookie", cookie.expose()),
                (None, true) => request.auth(&basic),
                (None, false) => request.auth(&mint),
            };
            if let Some(body) = body.clone() {
                request = request.json(body);
            }
            match effect {
                None => self.ctx.read(request),
                Some(effect) => self.ctx.write(effect, request),
            }
        };
        let refusal = |error: &anyhow::Error| matches!(status_of(error), Some(401 | 403));
        match attempt() {
            // Airflow 2 answers a refused credential with 403, as it does a
            // missing permission: only the form tells them apart.
            Err(error) if form && self.session.borrow().is_none() && refusal(&error) => {
                self.sign_in_form()?;
                self.ctx.cache().put(&remembered, &true, REMEMBER);
                attempt().map_err(|error| {
                    if !refusal(&error) {
                        return refused(error, path);
                    }
                    match error.downcast::<Failure>() {
                        Ok(failure) => failure
                            .hint(
                                "the password signs in, and the API still refuses it: the user's role lacks this \
                                 permission, or [api] auth_backends lists neither basic_auth nor session (ask the \
                                 Airflow admins to add airflow.api.auth.backend.basic_auth)",
                            )
                            .into(),
                        Err(error) => error,
                    }
                })
            }
            other => other.map_err(|error| refused(error, path)),
        }
    }

    /// Runs newest first: by `run_after` on Airflow 3; Airflow 2 has no
    /// such field and orders by its logical date, `execution_date`.
    pub(crate) fn newest_first(&self) -> Result<&'static str> {
        Ok(if self.v1()? {
            "order_by=-execution_date"
        } else {
            "order_by=-run_after"
        })
    }

    /// `DAG/latest` as the newest run of the DAG.
    pub(crate) fn resolve(&self, id: &mut Ref) -> Result<()> {
        if id.run != "latest" {
            return Ok(());
        }
        let answer = self.get(&format!(
            "{}/dagRuns?{}&limit=1",
            id.dag_path(),
            self.newest_first()?
        ))?;
        id.run = answer["dag_runs"][0]["dag_run_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| {
                let dag = &id.dag;
                Failure::not_found(format!("DAG {dag} has no runs, so {dag}/latest names none"))
                    .hint(format!("agent-cli airflow run create {dag}"))
            })?;
        Ok(())
    }

    /// Up to `limit` items of a list endpoint, 100 a page, and the total the
    /// server counted (for `[50 of 312; --limit N]`).
    pub(crate) fn list(
        &self,
        path: &str,
        query: &str,
        key: &str,
        limit: usize,
    ) -> Result<(Vec<Value>, Option<usize>)> {
        let mut items: Vec<Value> = Vec::new();
        let mut total = None;
        while items.len() < limit {
            let want = (limit - items.len()).min(PAGE);
            let joiner = if query.is_empty() { "" } else { "&" };
            let page = self.get(&format!(
                "{path}?{query}{joiner}limit={want}&offset={}",
                items.len()
            ))?;
            let got = page[key]
                .as_array()
                .cloned()
                .with_context(|| format!("no {key:?} in {path}"))?;
            total = page["total_entries"]
                .as_u64()
                .and_then(|total| usize::try_from(total).ok());
            // A server whose maximum_page_limit is under the page asked for
            // answers short pages: the total, when given, says what is left.
            let (short, empty) = (got.len() < want, got.is_empty());
            items.extend(got);
            if empty || total.map_or(short, |total| items.len() >= total) {
                break;
            }
        }
        Ok((items, total))
    }

    /// [`Self::list`] with `param` (`variable_key_pattern`, …) set to
    /// `pattern`: `%` and `_` are wildcards, matched anywhere in `field`,
    /// any case. Airflow 2 has no such filter, so there it runs here.
    // ponytail: on Airflow 2 matches among the first 1,000; page on if a
    // deployment has more.
    pub(crate) fn list_like(
        &self,
        path: &str,
        key: &str,
        (param, field): (&str, &str),
        pattern: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<Value>, Option<usize>)> {
        let Some(pattern) = pattern.map(str::trim) else {
            return self.list(path, "", key, limit);
        };
        if !self.v1()? {
            let query = format!("{param}={}", query_value(pattern));
            return self.list(path, &query, key, limit);
        }
        let (mut items, _) = self.list(path, "", key, 1000)?;
        items.retain(|item| like(pattern, item[field].as_str().unwrap_or_default()));
        let total = items.len();
        items.truncate(limit);
        Ok((items, Some(total)))
    }
}

/// SQL's `ILIKE '%pattern%'`: `%` is any run of characters, `_` one.
fn like(pattern: &str, text: &str) -> bool {
    fn matches(pattern: &[char], text: &[char]) -> bool {
        match (pattern.split_first(), text.split_first()) {
            (None, _) => text.is_empty(),
            (Some(('%', rest)), _) => {
                matches(rest, text) || (!text.is_empty() && matches(pattern, &text[1..]))
            }
            (Some((want, rest)), Some((got, more))) if *want == '_' || want == got => {
                matches(rest, more)
            }
            _ => false,
        }
    }
    let pattern: Vec<char> = format!("%{}%", pattern.to_lowercase()).chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();
    matches(&pattern, &text)
}

/// A non-empty string, owned.
pub(crate) fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// A service timestamp as every command prints one.
pub(crate) fn stamp(value: &Value) -> Option<String> {
    value.as_str().filter(|raw| !raw.is_empty()).map(utc)
}

/// Whole seconds from `start` to `end`, when both are times.
pub(crate) fn seconds(start: &Value, end: &Value) -> Option<i64> {
    let at = |value: &Value| OffsetDateTime::parse(value.as_str()?, &Rfc3339).ok();
    Some((at(end)? - at(start)?).whole_seconds())
}

/// `[50 of 312; --limit N]` when the server counted more than printed.
pub(crate) fn note_more(ctx: &Ctx, shown: usize, total: Option<usize>) {
    if let Some(total) = total.filter(|total| *total > shown) {
        ctx.note(format!("[{shown} of {total}; --limit N]"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DOMAIN;
    use crate::testing::{CONFIG, airflow_with};
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};
    use serde_json::json;

    #[test]
    fn a_sign_in_under_a_wrong_prefix_or_a_list_less_answer_is_named_not_empty() {
        let password = "[[airflow.instance]]\nname = \"dev\"\nbase_url = \"https://airflow.contoso.example/airflow\"\nusername = \"agent\"\npassword_env = \"AIRFLOW_TOKEN\"\n";
        let (outcome, _) = crate::testing::airflow_with(
            password,
            &["airflow", "dag", "list"],
            vec![Answer::status(405, "Method Not Allowed")],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains("base_url is wrong"),
            "{}",
            outcome.stderr
        );
        let (outcome, _) = crate::testing::airflow(
            &["airflow", "dag", "list"],
            vec![Answer::json(&serde_json::json!({}))],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stdout.is_empty(),
            "never a confident []: {outcome:?}"
        );
        assert!(
            outcome.stderr.contains("no \"dags\" in dags"),
            "{}",
            outcome.stderr
        );
    }
    #[test]
    fn like_matches_anywhere_with_sql_wildcards_and_any_case() {
        for (pattern, text, want) in [
            ("orders", "e2e_orders_bucket", true),
            ("ORD%BUCK", "e2e_orders_bucket", true),
            ("e2e_", "e2eXapi", true),
            ("orders", "customers", false),
            ("", "anything", true),
        ] {
            assert_eq!(like(pattern, text), want, "{pattern} {text}");
        }
    }

    #[test]
    fn a_token_goes_only_under_the_configured_base_url() {
        for (base, url) in [
            (
                "https://airflow.contoso.example",
                "https://airflow.contoso.example/api/v2/dags",
            ),
            ("http://localhost:8080", "http://localhost:8080/auth/token"),
            (
                "https://contoso.example/airflow",
                "https://contoso.example/airflow/api/v2/x",
            ),
        ] {
            assert!(same_origin(base, url), "{url}");
        }
        for (base, url) in [
            (
                "https://airflow.contoso.example",
                "https://airflow.contoso.example.evil.example/x",
            ),
            (
                "https://airflow.contoso.example",
                "https://airflow.contoso.example@evil.example/x",
            ),
            (
                "https://airflow.contoso.example",
                "http://airflow.contoso.example/x",
            ),
            (
                "https://airflow.contoso.example",
                "https://airflow.contoso.example:8443/x",
            ),
            (
                "https://contoso.example/airflow",
                "https://contoso.example/airflow2/x",
            ),
            (
                "https://contoso.example/airflow",
                "https://contoso.example/other",
            ),
            ("http://localhost:8080", "http://localhost:80801/x"),
        ] {
            assert!(!same_origin(base, url), "{url}");
        }
    }

    #[test]
    fn a_ui_url_decodes_to_the_same_pieces() {
        assert_eq!(
            decode("scheduled__2026-09-28T00%3A00%3A00%2B00%3A00"),
            "scheduled__2026-09-28T00:00:00+00:00"
        );
        assert_eq!(decode("100%"), "100%");
    }

    #[test]
    fn a_broken_instance_is_exit_3_naming_it() {
        for (body, want) in [
            (
                "base_url = \"http://airflow.contoso.example\"\ntoken_env = \"T\"",
                "plain http only to localhost",
            ),
            (
                "base_url = \"https://airflow.contoso.example/api/v2\"\ntoken_env = \"T\"",
                "ends in /api/v2",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"\nusername = \"a\"",
                "username needs",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"",
                "no credential",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"\ntoken_env = \"T\"\ntoken_cmd = \"x\"",
                "give one of token",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"\ntoken_env = \"T\"\nbogus = 1",
                "unknown field",
            ),
        ] {
            let config = format!("[[airflow.instance]]\nname = \"prod\"\n{body}\n");
            let (outcome, _) = airflow_with(&config, &["airflow", "dag", "list"], vec![]);
            assert_eq!(outcome.code, 3, "{body}: {outcome:?}");
            assert!(outcome.stderr.contains(want), "{body}: {}", outcome.stderr);
        }
    }

    #[test]
    fn a_password_signs_in_once_and_again_after_a_401_and_the_jwt_never_prints() {
        let config = "[[airflow.instance]]\nname = \"dev\"\nbase_url = \"http://localhost:8080\"\n\
                      username = \"agent\"\npassword_env = \"AIRFLOW_PASSWORD\"\napi = \"v2\"\n";
        let page = json!({"dags": [], "total_entries": 0});
        let transport = FakeTransport::answering([
            Answer::status(201, r#"{"access_token":"eyJhbGciOi.first-jwt.sig1"}"#),
            Answer::status(401, r#"{"detail":"Token expired"}"#),
            Answer::status(201, r#"{"access_token":"eyJhbGciOi.second-jwt.sig2"}"#),
            Answer::json(&page),
        ]);
        let setup = Setup::fake(transport.clone())
            .with_config(config)
            .with_env("AIRFLOW_PASSWORD", "dev-password-1");
        let outcome = run(&[DOMAIN], &["airflow", "dag", "list"], setup);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        let calls: Vec<(&str, &str, Option<&str>)> = sent
            .iter()
            .map(|s| (s.method.wire(), s.url.as_str(), s.authorization.as_deref()))
            .collect();
        let dags = "http://localhost:8080/api/v2/dags?order_by=dag_id&limit=50&offset=0";
        assert_eq!(
            calls,
            [
                ("POST", "http://localhost:8080/auth/token", None),
                ("GET", dags, Some("Bearer eyJhbGciOi.first-jwt.sig1")),
                ("POST", "http://localhost:8080/auth/token", None),
                ("GET", dags, Some("Bearer eyJhbGciOi.second-jwt.sig2")),
            ]
        );
        assert!(sent[0].method.is_read(), "the sign-in is a read");
        assert_eq!(
            sent[0].body,
            Some(json!({"username": "agent", "password": "dev-password-1"}))
        );
        for secret in ["first-jwt", "second-jwt", "dev-password-1"] {
            assert!(!outcome.stdout.contains(secret) && !outcome.stderr.contains(secret));
        }

        let refused =
            FakeTransport::answering([Answer::status(401, r#"{"detail":"Invalid credentials"}"#)]);
        let setup = Setup::fake(refused)
            .with_config(config)
            .with_env("AIRFLOW_PASSWORD", "wrong-password-1");
        let outcome = run(&[DOMAIN], &["airflow", "dag", "list"], setup);
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Invalid credentials")
                && outcome
                    .stderr
                    .contains("`agent-cli doctor airflow` checks it"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn refusals_read_as_the_next_step() {
        for (answer, code, hint) in [
            (
                Answer::status(401, r#"{"detail":"Invalid token"}"#),
                3,
                "`agent-cli doctor airflow` checks it",
            ),
            (
                Answer::status(403, r#"{"detail":"Forbidden"}"#),
                1,
                "lacks this permission",
            ),
            (
                Answer::status(307, "").with_header("Location", "https://login.contoso.example/"),
                3,
                "base_url is wrong",
            ),
        ] {
            // A 401 is retried once with a fresh token, so it is answered twice.
            let answers = if answer.status == 401 {
                vec![answer.clone(), answer]
            } else {
                vec![answer]
            };
            let (outcome, _) = airflow_with(CONFIG, &["airflow", "dag", "list"], answers);
            assert_eq!(outcome.code, code, "{outcome:?}");
            assert!(outcome.stderr.contains(hint), "{}", outcome.stderr);
        }
    }

    #[test]
    fn two_instances_and_no_flag_is_exit_2_naming_both_before_any_request() {
        let config = format!(
            "{CONFIG}\n[[airflow.instance]]\nname = \"qa\"\nbase_url = \"https://qa.contoso.example\"\ntoken_env = \"T\"\n"
        );
        let (outcome, transport) = airflow_with(&config, &["airflow", "dag", "list"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("prod, qa"), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());
    }
}
