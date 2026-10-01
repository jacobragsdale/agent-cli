//! Airflow 3 over its REST API (`/api/v2`): the `[airflow]` section, the
//! token, the one door every request goes through, and the ids every command
//! takes.
//!
//! Core's `host_under` does not fit a configured server (it wants https with
//! no port, and a compose Airflow is `http://localhost:8080`), but every URL
//! this crate sends is built from `base_url`: paging is by offset, the log
//! token is a query value, and Airflow never hands back a URL to follow. So
//! the door attaches the token only to URLs that start with `base_url/`
//! exactly, and `base_url` itself is checked when the section loads.

use std::cell::RefCell;

use agent_cli_core::{
    Config, Credential, Ctx, Effect, Exit, Failure, Method, Request, Response, Secret,
    percent_encode, pick, utc,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Airflow caps a page at `[api] maximum_page_limit`, 100 unless raised.
const PAGE: usize = 100;
/// Where the Helm chart's docs install Airflow, and so where its
/// KubernetesExecutor starts task pods unless `k8s_namespace` says otherwise.
const CHART_NAMESPACE: &str = "airflow";

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
}

pub(crate) enum Auth {
    /// `POST {base_url}/auth/token`: the Simple, FAB and Keycloak managers.
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
    pub(crate) auth: Auth,
    pub(crate) read_only: bool,
    /// The `[[k8s.scope]]` its KubernetesExecutor pods run in.
    pub(crate) k8s_scope: Option<String>,
    pub(crate) k8s_namespace: String,
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
                    .hint("fix it; config.example.toml shows every key")
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
                .hint("add an [[airflow.instance]]; config.example.toml shows the keys")
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
            auth,
            read_only: self.read_only,
            k8s_scope,
            k8s_namespace: self
                .k8s_namespace
                .map(|namespace| namespace.trim().to_owned())
                .filter(|namespace| !namespace.is_empty())
                .unwrap_or_else(|| CHART_NAMESPACE.to_owned()),
        })
    }
}

/// `https://host[:port][/prefix]`, or plain http to this machine (a compose
/// Airflow). Anything a URL parser might read two ways is refused.
fn check_base_url(raw: &str) -> Result<String, String> {
    let base = raw.trim().trim_end_matches('/');
    if let Some(server) = base.strip_suffix("/api/v2") {
        return Err(format!(
            "base_url {raw:?} ends in /api/v2; give the server without it: {server:?}"
        ));
    }
    let wrong = || {
        format!(
            "base_url {raw:?} is not https://HOST[:PORT][/PREFIX] (plain http only to localhost)"
        )
    };
    let (scheme, rest) = base.split_once("://").ok_or_else(wrong)?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let odd_path = |c: char| !(c.is_ascii_alphanumeric() || "-._~/%".contains(c));
    let odd_host = |c: char| !(c.is_ascii_alphanumeric() || "-.:[]".contains(c));
    if authority.is_empty() || authority.contains(odd_host) || path.contains(odd_path) {
        return Err(wrong());
    }
    let host = match authority.split_once(']') {
        Some((v6, _)) => format!("{v6}]"),
        None => authority.split(':').next().unwrap_or_default().to_owned(),
    };
    let port = authority[host.len()..].strip_prefix(':');
    if port.is_some_and(|port| port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()))
        || !(authority.len() == host.len() || port.is_some())
    {
        return Err(wrong());
    }
    let local = matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]");
    match scheme {
        "https" => Ok(base.to_owned()),
        "http" if local => Ok(base.to_owned()),
        _ => Err(wrong()),
    }
}

/// True when `url` is on `base`'s server under its path: it starts with
/// `base` and a `/`. The one check before a token is attached.
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

/// What a command's positional names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Want {
    Dag,
    Run,
    Task,
}

impl Want {
    fn noun(self) -> &'static str {
        match self {
            Self::Dag => "DAG",
            Self::Run => "run",
            Self::Task => "task instance",
        }
    }

    fn shape(self) -> &'static str {
        match self {
            Self::Dag => "DAG (etl_nightly)",
            Self::Run => {
                "DAG/RUN (etl_nightly/scheduled__2026-09-28T00:00:00+00:00, or DAG/latest)"
            }
            Self::Task => {
                "DAG/RUN/TASK[:MAP][/TRY] (etl_nightly/latest/load_orders, …/load_orders:3/2)"
            }
        }
    }

    fn url_shape(self) -> &'static str {
        match self {
            Self::Dag => "/dags/DAG",
            Self::Run => "/dags/DAG/runs/RUN",
            Self::Task => "/dags/DAG/runs/RUN/tasks/TASK",
        }
    }
}

/// A DAG, run or task instance, as its pieces.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Ref {
    pub(crate) dag: String,
    pub(crate) run: String,
    pub(crate) task: String,
    pub(crate) map: Option<i64>,
    /// The try the id names, when it names one.
    pub(crate) attempt: Option<i64>,
}

impl Ref {
    /// `raw` as `want`'s id, with `--dag` and `--run` standing in for its
    /// leading pieces. The DAG comes off the front and `TASK[:MAP][/TRY]` off
    /// the back, so a custom run id holding `/` still parses.
    pub(crate) fn parse(
        raw: &str,
        want: Want,
        dag: Option<&str>,
        run: Option<&str>,
    ) -> Result<Self, String> {
        let mut parts: Vec<&str> = raw.split('/').collect();
        if parts
            .iter()
            .any(|part| part.trim().is_empty() || matches!(*part, "." | ".."))
        {
            return Err(format!("{raw:?} is not a {} id", want.noun()));
        }
        let mut id = Self::default();
        let missing = |what: &str| format!("{raw:?} names no {what}; give {}", want.shape());
        match want {
            Want::Dag => {
                if parts.len() != 1 {
                    return Err(format!("{raw:?} is not a DAG id (it holds a /)"));
                }
                id.dag = parts[0].to_owned();
                disagree(raw, &id.dag, dag, "dag")?;
            }
            Want::Run => {
                if parts.len() == 1 {
                    id.dag = dag.ok_or_else(|| missing("DAG"))?.to_owned();
                    id.run = parts[0].to_owned();
                } else {
                    id.dag = parts[0].to_owned();
                    id.run = parts[1..].join("/");
                    disagree(raw, &id.dag, dag, "dag")?;
                }
            }
            Want::Task => {
                // The pieces the id must carry in front of the task.
                let front = 2 - usize::from(dag.is_some()) - usize::from(run.is_some());
                let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
                if parts.len() > front + 1 && parts.last().is_some_and(|last| digits(last)) {
                    id.attempt = parts.pop().and_then(|attempt| attempt.parse().ok());
                }
                let task = parts.pop().unwrap_or_default();
                match task.split_once(':') {
                    Some((task, map)) if digits(map) && !map.is_empty() => {
                        id.task = task.to_owned();
                        id.map = map.parse().ok();
                    }
                    Some(_) => return Err(format!("{raw:?}: a map index is :N, a number")),
                    None => id.task = task.to_owned(),
                }
                match (parts.as_slice(), dag, run) {
                    ([], Some(dag), Some(run)) => {
                        id.dag = dag.to_owned();
                        id.run = run.to_owned();
                    }
                    ([held], Some(dag), Some(run)) => {
                        disagree(raw, held, Some(run), "run")?;
                        id.dag = dag.to_owned();
                        id.run = run.to_owned();
                    }
                    ([held], Some(dag), None) => {
                        id.dag = dag.to_owned();
                        id.run = (*held).to_owned();
                    }
                    ([held], None, Some(run)) => {
                        id.dag = (*held).to_owned();
                        id.run = run.to_owned();
                    }
                    ([first, rest @ ..], dag, run) if !rest.is_empty() => {
                        id.dag = (*first).to_owned();
                        id.run = rest.join("/");
                        disagree(raw, &id.dag, dag, "dag")?;
                        disagree(raw, &id.run, run, "run")?;
                    }
                    _ => return Err(missing("DAG and run")),
                }
                if id.task.is_empty() {
                    return Err(missing("task"));
                }
            }
        }
        Ok(id)
    }

    /// `DAG/RUN`: what the run commands take.
    pub(crate) fn run_id(&self) -> String {
        format!("{}/{}", self.dag, self.run)
    }

    pub(crate) fn dag_path(&self) -> String {
        format!("dags/{}", segment(&self.dag))
    }

    pub(crate) fn run_path(&self) -> String {
        format!("{}/dagRuns/{}", self.dag_path(), segment(&self.run))
    }

    /// `…/taskInstances/TASK[/MAP]`.
    pub(crate) fn ti_path(&self) -> String {
        let mut path = format!("{}/taskInstances/{}", self.run_path(), segment(&self.task));
        if let Some(map) = self.map {
            path.push_str(&format!("/{map}"));
        }
        path
    }
}

/// Exit 2 when a flag names a different piece than the ref holds.
fn disagree(raw: &str, held: &str, flag: Option<&str>, what: &str) -> Result<(), String> {
    match flag {
        Some(flag) if !held.is_empty() && flag != held => Err(format!(
            "{raw} names {what} {held}, and --{what} says {flag}"
        )),
        _ => Ok(()),
    }
}

/// A task instance's id from the API's own record: `DAG/RUN/TASK[:MAP]`,
/// and `/TRY` when `with_try` and it has run, so it pastes into `task logs`
/// for that try.
pub(crate) fn ti_id(ti: &Value, with_try: bool) -> String {
    let mut id = format!(
        "{}/{}/{}",
        ti["dag_id"].as_str().unwrap_or_default(),
        ti["dag_run_id"].as_str().unwrap_or_default(),
        ti["task_id"].as_str().unwrap_or_default()
    );
    if let Some(map) = ti["map_index"].as_i64().filter(|map| *map >= 0) {
        id.push_str(&format!(":{map}"));
    }
    if let Some(attempt) = ti["try_number"]
        .as_i64()
        .filter(|attempt| with_try && *attempt > 0)
    {
        id.push_str(&format!("/{attempt}"));
    }
    id
}

/// One instance, and the token for it, minted at most once per command.
pub(crate) struct Client<'a> {
    pub(crate) ctx: &'a Ctx,
    pub(crate) instance: &'a Instance,
    /// The `Authorization` value. JWTs stay in memory, never on disk.
    token: RefCell<Option<Secret>>,
}

impl<'a> Client<'a> {
    pub(crate) fn new(ctx: &'a Ctx, instance: &'a Instance) -> Self {
        Self {
            ctx,
            instance,
            token: RefCell::new(None),
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

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}/api/v2/{path}", self.instance.base_url)
    }

    pub(crate) fn get(&self, path: &str) -> Result<Value> {
        self.send(Method::Get, path, None, None)?.json()
    }

    /// A read that needs no credential (version, health), so doctor can tell
    /// a wrong `base_url` from a wrong password.
    pub(crate) fn public(&self, path: &str) -> Result<Value> {
        let request = Request::get(self.url(path)).header("Accept", "application/json");
        self.ctx
            .read(request)
            .map_err(|error| refused(error, path))?
            .json()
    }

    /// A POST that only reads: a clear's server-side dry run.
    pub(crate) fn preview(&self, path: &str, body: Value) -> Result<Value> {
        self.send(Method::Query, path, Some(body), None)?.json()
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
        self.send(method, path, Some(body), Some(effect))?.json()
    }

    /// The one door: the token only under `base_url`, and Airflow's refusals
    /// read as the next step to take.
    fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        effect: Option<Effect>,
    ) -> Result<Response> {
        let url = self.url(path);
        if !same_origin(&self.instance.base_url, &url) {
            bail!("refusing to send the Airflow token to {url}");
        }
        let mint = |fresh: bool| self.authorization(fresh);
        let mut request = Request::new(method, &url)
            .header("Accept", "application/json")
            .auth(&mint);
        if let Some(body) = body {
            request = request.json(body);
        }
        match effect {
            None => self.ctx.read(request),
            Some(effect) => self.ctx.write(effect, request),
        }
        .map_err(|error| refused(error, path))
    }

    /// `Bearer …`: minted once, and again after a 401.
    fn authorization(&self, fresh: bool) -> Result<Secret> {
        if !fresh && let Some(token) = self.token.borrow().clone() {
            return Ok(token);
        }
        let bearer = match &self.instance.auth {
            Auth::Token(token) => token.resolve(self.ctx)?,
            Auth::Password { username, password } => {
                self.sign_in(username, &password.resolve(self.ctx)?)?
            }
        };
        let token = Secret::new(format!("Bearer {}", bearer.expose()));
        *self.token.borrow_mut() = Some(token.clone());
        Ok(token)
    }

    /// A JWT from `/auth/token`. A read (the POST changes nothing), so it
    /// also works under `--dry-run` and `AGENT_CLI_READ_ONLY`.
    fn sign_in(&self, username: &str, password: &Secret) -> Result<Secret> {
        let url = format!("{}/auth/token", self.instance.base_url);
        let request = Request::query(
            &url,
            json!({"username": username, "password": password.expose()}),
        )
        .header("Accept", "application/json");
        let response = self.ctx.read(request).map_err(|error| {
            match error.downcast::<Failure>() {
                Ok(failure) if matches!(failure.status, Some(400 | 401 | 403)) => {
                    Failure::setup(failure.message)
                        .hint(format!(
                            "check username and the password for instance {:?}; `agent-cli doctor airflow` checks it",
                            self.instance.name
                        ))
                        .into()
                }
                Ok(failure) => refused(failure.into(), "auth/token"),
                Err(error) => error,
            }
        })?;
        let token = response.json()?["access_token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .map(Secret::new)
            .with_context(|| format!("{url} answered without an access_token"))?;
        Ok(token)
    }

    /// `DAG/latest` as the newest run of the DAG by `run_after`.
    pub(crate) fn resolve(&self, id: &mut Ref) -> Result<()> {
        if id.run != "latest" {
            return Ok(());
        }
        let answer = self.get(&format!(
            "{}/dagRuns?order_by=-run_after&limit=1",
            id.dag_path()
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
            let got = page[key].as_array().cloned().unwrap_or_default();
            total = page["total_entries"]
                .as_u64()
                .and_then(|total| usize::try_from(total).ok());
            let short = got.len() < want;
            items.extend(got);
            if short || total.is_some_and(|total| items.len() >= total) {
                break;
            }
        }
        Ok((items, total))
    }
}

/// Airflow's refusals as the next step: a 401 that a fresh token did not fix
/// is the credential, a 403 the role, a redirect the `base_url`, and a 404
/// names the list to look in.
fn refused(error: anyhow::Error, path: &str) -> anyhow::Error {
    let mut failure = match error.downcast::<Failure>() {
        Ok(failure) => failure,
        Err(error) => return error,
    };
    match failure.status {
        Some(401) => {
            failure.hint = Some(
                "the Airflow credential was refused; `agent-cli doctor airflow` checks it"
                    .to_owned(),
            );
        }
        Some(403) => {
            failure.hint =
                Some("the Airflow role behind this credential lacks this permission".to_owned());
        }
        Some(300..=399) => {
            failure.exit = Exit::Setup;
            failure.hint = Some(
                "base_url is wrong (its scheme or path prefix); fix it under [[airflow.instance]]"
                    .to_owned(),
            );
        }
        Some(404) if failure.message.contains("is mapped") => {
            failure.exit = Exit::Usage;
            failure.hint = Some(
                "name the mapped task as TASK:N; its map indexes: agent-cli airflow task list DAG/RUN"
                    .to_owned(),
            );
        }
        Some(404) => {
            let list = if path.contains("/taskInstances") {
                "agent-cli airflow task list DAG/RUN"
            } else if path.contains("/dagRuns") {
                "agent-cli airflow run list --dag DAG"
            } else if path.starts_with("importErrors") {
                "agent-cli airflow import-error list"
            } else {
                "agent-cli airflow dag list"
            };
            failure.hint = Some(list.to_owned());
        }
        _ => {}
    }
    failure.into()
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
    fn base_url_is_https_or_http_to_this_machine_and_never_ambiguous() {
        for (raw, want) in [
            (
                "https://airflow.contoso.example/",
                "https://airflow.contoso.example",
            ),
            ("http://localhost:8080", "http://localhost:8080"),
            ("http://127.0.0.1:8080/", "http://127.0.0.1:8080"),
            ("http://[::1]:8080", "http://[::1]:8080"),
            (
                "https://contoso.example:8443/d-1a2b3c",
                "https://contoso.example:8443/d-1a2b3c",
            ),
        ] {
            assert_eq!(check_base_url(raw).as_deref(), Ok(want), "{raw}");
        }
        for raw in [
            "http://airflow.contoso.example",
            "airflow.contoso.example",
            "https://user@airflow.contoso.example",
            "https://airflow.contoso.example\\@evil",
            "https://airflow.contoso.example:",
            "https://airflow.contoso.example:80x",
            "https://airflow.contoso.example/x?y=1",
            "ftp://localhost",
            "http://localhost.evil.example",
        ] {
            assert!(check_base_url(raw).is_err(), "{raw}");
        }
        let api = check_base_url("https://airflow.contoso.example/api/v2/").unwrap_err();
        assert!(api.contains("\"https://airflow.contoso.example\""), "{api}");
    }

    #[test]
    fn ids_take_the_dag_off_the_front_and_the_task_off_the_back() {
        let task =
            |raw: &str, dag: Option<&str>, run: Option<&str>| Ref::parse(raw, Want::Task, dag, run);
        let want = |dag: &str, run: &str, task: &str, map: Option<i64>, attempt: Option<i64>| {
            Ok(Ref {
                dag: dag.into(),
                run: run.into(),
                task: task.into(),
                map,
                attempt,
            })
        };
        let run = "scheduled__2026-09-28T00:00:00+00:00";
        assert_eq!(
            task(&format!("etl/{run}/load_orders:3/2"), None, None),
            want("etl", run, "load_orders", Some(3), Some(2))
        );
        assert_eq!(
            task("etl/custom/with/slash/load/2", None, None),
            want("etl", "custom/with/slash", "load", None, Some(2)),
            "a run id holding / still parses"
        );
        assert_eq!(
            task("etl/r1/123", None, None),
            want("etl", "r1", "123", None, None),
            "three pieces are always DAG/RUN/TASK"
        );
        assert_eq!(
            task("load", Some("etl"), Some("r1")),
            want("etl", "r1", "load", None, None)
        );
        assert_eq!(
            task("load/2", Some("etl"), Some("r1")),
            want("etl", "r1", "load", None, Some(2))
        );
        assert_eq!(
            task("r1/load", Some("etl"), None),
            want("etl", "r1", "load", None, None)
        );
        assert_eq!(
            task("etl/r1/load", Some("etl"), Some("r1")),
            want("etl", "r1", "load", None, None),
            "a flag that agrees is fine"
        );
        for (raw, dag, run) in [
            ("etl/r1/load", Some("other"), None),
            ("etl/r1/load", None, Some("r2")),
            ("load", None, None),
            ("r1/load", None, None),
            ("etl/r1/load:x", None, None),
            ("etl//load", None, None),
            ("etl/../load", None, None),
        ] {
            assert!(task(raw, dag, run).is_err(), "{raw} {dag:?} {run:?}");
        }
        assert_eq!(
            Ref::parse("etl/latest", Want::Run, None, None)
                .unwrap()
                .run_id(),
            "etl/latest"
        );
        assert_eq!(
            Ref::parse("r1", Want::Run, Some("etl"), None)
                .unwrap()
                .run_id(),
            "etl/r1"
        );
        assert!(Ref::parse("r1", Want::Run, None, None).is_err());
        assert!(Ref::parse("etl/r1", Want::Run, Some("other"), None).is_err());
        assert!(Ref::parse("etl/r1", Want::Dag, None, None).is_err());
        let id = Ref::parse(&format!("etl/{run}/load:3"), Want::Task, None, None).unwrap();
        assert_eq!(
            id.ti_path(),
            "dags/etl/dagRuns/scheduled__2026-09-28T00%3A00%3A00%2B00%3A00/taskInstances/load/3"
        );
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
                      username = \"agent\"\npassword_env = \"AIRFLOW_PASSWORD\"\n";
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
