//! Control-M over the Automation API of an on-prem Enterprise Manager: the
//! `[controlm]` section, the one door every request goes through, and the
//! row helpers every command shares.
//!
//! Written from BMC's spec (the client generated from it in GitHub's
//! controlm/ctm-python-client, API 9.22.30) without an Enterprise Manager to
//! try it on. Each `VERIFY(work)` names an assumption to check against a real
//! one; docs/plans/controlm.md says how.

use std::cell::RefCell;

use agent_cli_core::{
    Config, Credential, Ctx, Effect, Exit, Failure, Method, Op, Request, Response, Secret, When,
    check_base_url, percent_encode, pick, same_origin, utc, utc_time,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

/// `[controlm]` in config.toml.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    instance: Vec<Raw>,
}

/// One `[[controlm.instance]]` as written.
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
    utc_offset: Option<String>,
}

pub(crate) enum Auth {
    /// `POST {base_url}/session/login`: a session token, sent as
    /// `Authorization: Bearer`. Works on every version, with the same user
    /// and password as Control-M's web client.
    Password {
        username: String,
        password: Credential,
    },
    /// An API token (Enterprise Manager 9.0.21 and later), sent as
    /// `x-api-key`.
    Token(Credential),
}

/// One Enterprise Manager, checked.
pub(crate) struct Instance {
    pub(crate) name: String,
    /// The Automation API endpoint, `https://HOST:8443/automation-api`, with
    /// no trailing slash.
    pub(crate) base_url: String,
    pub(crate) auth: Auth,
    pub(crate) read_only: bool,
    /// The clock of the Control-M/Servers, which every time the API prints
    /// and takes is on.
    pub(crate) offset: UtcOffset,
}

impl Instance {
    /// The kind of credential and where it comes from, never its value.
    pub(crate) fn auth_source(&self) -> String {
        match &self.auth {
            Auth::Password { username, password } => {
                format!("password for {username} ({})", password.source())
            }
            Auth::Token(token) => format!("API token ({})", token.source()),
        }
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
                return Err("an API token needs no username; drop it".to_owned());
            }
            (None, None, None) => {
                return Err(
                    "no credential: give username with password_env (or password_cmd), or token_env (or token_cmd)"
                        .to_owned(),
                );
            }
        };
        let offset = match self.utc_offset.as_deref().map(str::trim) {
            None | Some("") => UtcOffset::UTC,
            Some(raw) => parse_offset(raw)?,
        };
        Ok(Instance {
            name: self.name,
            base_url,
            auth,
            read_only: self.read_only,
            offset,
        })
    }
}

// ponytail: one fixed offset is wrong for half the year where the servers keep
// summer time; a time zone database (a new dependency) is the upgrade.
fn parse_offset(raw: &str) -> Result<UtcOffset, String> {
    let wrong = || format!("utc_offset {raw:?} is not +HH:MM or -HH:MM");
    if raw == "Z" {
        return Ok(UtcOffset::UTC);
    }
    let (sign, rest) = match raw.split_at_checked(1) {
        Some(("+", rest)) => (1, rest),
        Some(("-", rest)) => (-1, rest),
        _ => return Err(wrong()),
    };
    let (hours, minutes) = rest.split_once(':').ok_or_else(wrong)?;
    let hours: i8 = hours.parse().map_err(|_| wrong())?;
    let minutes: i8 = minutes.parse().map_err(|_| wrong())?;
    UtcOffset::from_hms(sign * hours, sign * minutes, 0).map_err(|_| wrong())
}

/// Which instance a command talks to.
#[derive(Clone, Debug, clap::Args)]
pub struct At {
    /// The [[controlm.instance]] name; defaults to the only one
    #[arg(long)]
    pub(crate) instance: Option<String>,
}

/// Every configured instance.
pub(crate) struct ControlM {
    pub(crate) instances: Vec<Instance>,
}

impl ControlM {
    /// The `[controlm]` section; exit 3 naming the file when it is wrong.
    pub(crate) fn load(config: &Config) -> Result<Self> {
        let section: Section = config.section("controlm")?;
        let path = config.path().display().to_string();
        let mut instances: Vec<Instance> = Vec::new();
        for raw in section.instance {
            let name = raw.name.clone();
            let checked = if instances.iter().any(|earlier| earlier.name == name) {
                Err("is named twice".to_owned())
            } else {
                raw.check()
            };
            let instance = checked.map_err(|why| {
                Failure::setup(format!("[[controlm.instance]] {name:?} in {path}: {why}"))
                    .hint("fix it; `agent-cli config example controlm` shows every key")
            })?;
            instances.push(instance);
        }
        Ok(Self { instances })
    }

    /// A client for the instance `--instance` names, or the only one.
    pub(crate) fn open<'a>(&'a self, ctx: &'a Ctx, at: &At) -> Result<Client<'a>> {
        let instance = pick(
            "Control-M instance",
            "--instance",
            at.instance.as_deref(),
            &self.instances,
            |instance| &instance.name,
        )
        .map_err(|failure| match failure.exit {
            Exit::Usage => failure.hint("name one with --instance"),
            _ => failure.hint(
                "add a [[controlm.instance]]; `agent-cli config example controlm` shows the keys",
            ),
        })?;
        Ok(Client::new(ctx, instance))
    }
}

/// One Enterprise Manager and its credential, resolved at most once per
/// command.
pub(crate) struct Client<'a> {
    pub(crate) ctx: &'a Ctx,
    pub(crate) instance: &'a Instance,
    /// `Bearer …` from a session login, in memory only.
    session: RefCell<Option<Secret>>,
    /// The API token, once resolved.
    key: RefCell<Option<Secret>>,
    /// The Enterprise Manager's version, as a session login reports it.
    pub(crate) version: RefCell<Option<String>>,
}

impl<'a> Client<'a> {
    pub(crate) fn new(ctx: &'a Ctx, instance: &'a Instance) -> Self {
        Self {
            ctx,
            instance,
            session: RefCell::new(None),
            key: RefCell::new(None),
            version: RefCell::new(None),
        }
    }

    /// Exit 2 on an instance marked `read_only`, before anything is sent.
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

    pub(crate) fn get(&self, path: &str) -> Result<Value> {
        self.send(Method::Get, path, None, None, "application/json")?
            .json()
    }

    /// A read as plain text: a job's output and log.
    // VERIFY(work): the spec offers application/json and text/plain for
    // output and log. If text/plain still comes back as a JSON string
    // ("\"line 1\\nline 2\""), decode it here.
    pub(crate) fn text(&self, path: &str) -> Result<String> {
        Ok(self.send(Method::Get, path, None, None, "text/plain")?.body)
    }

    /// A `POST` that changes something, which core checks against
    /// `--dry-run`, read-only mode and `--yes`.
    pub(crate) fn change(&self, effect: Effect, path: &str, body: Value) -> Result<Value> {
        self.writable()?;
        self.send(
            Method::Post,
            path,
            Some(body),
            Some(effect),
            "application/json",
        )?
        .json()
    }

    /// The one door: the credential only under `base_url`, and Control-M's
    /// refusals read as the next step.
    fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        effect: Option<Effect>,
        accept: &'static str,
    ) -> Result<Response> {
        let call = Call {
            client: self,
            method,
            url: format!("{}/{path}", self.instance.base_url),
            body,
            accept,
        };
        let answer = match effect {
            None => self.ctx.read(call),
            Some(effect) => self.ctx.write(effect, call),
        };
        answer.map_err(|error| refused(error, path))
    }

    /// The API token, resolved once.
    fn key(&self, token: &Credential) -> Result<Secret> {
        if let Some(key) = self.key.borrow().clone() {
            return Ok(key);
        }
        let key = token.resolve(self.ctx)?;
        *self.key.borrow_mut() = Some(key.clone());
        Ok(key)
    }

    /// `Bearer …` from `POST /session/login`: once, and again after a 401
    /// (a session idles out after 30 minutes by default). A read, as the
    /// login changes nothing, so it also works under `--dry-run` and
    /// `AGENT_CLI_READ_ONLY`.
    // VERIFY(work): each command logs in once and never logs out, so a
    // session lives until it idles out. If the Enterprise Manager caps
    // sessions per user, send `POST /session/logout` when a command ends, or
    // use an API token.
    fn bearer(&self, fresh: bool) -> Result<Secret> {
        if !fresh && let Some(bearer) = self.session.borrow().clone() {
            return Ok(bearer);
        }
        let Auth::Password { username, password } = &self.instance.auth else {
            bail!("a session login needs a username and password");
        };
        let url = format!("{}/session/login", self.instance.base_url);
        let request = Request::query(
            &url,
            json!({"username": username, "password": password.resolve(self.ctx)?.expose()}),
        )
        .header("Accept", "application/json");
        let answer = self
            .ctx
            .read(request)
            .map_err(|error| match error.downcast::<Failure>() {
                Ok(failure) if matches!(failure.status, Some(400 | 401 | 403)) => {
                    Failure::setup(failure.message)
                        .hint(format!(
                            "check username and the password of instance {:?}; `agent-cli doctor controlm` checks it",
                            self.instance.name
                        ))
                        .into()
                }
                Ok(failure) => refused(failure.into(), "session/login"),
                Err(error) => refused(error, "session/login"),
            })?
            .json()?;
        // The 9.0.21 docs show {"username", "token", "version"}.
        let token = answer["token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .with_context(|| format!("{url} answered without a token"))?;
        *self.version.borrow_mut() = text(&answer["version"]);
        let bearer = Secret::new(format!("Bearer {token}"));
        *self.session.borrow_mut() = Some(bearer.clone());
        Ok(bearer)
    }
}

/// One request, its credential attached when it is performed: after core's
/// read-only and `--dry-run` checks, and only under `base_url`.
struct Call<'c, 'a> {
    client: &'c Client<'a>,
    method: Method,
    url: String,
    body: Option<Value>,
    accept: &'static str,
}

impl Call<'_, '_> {
    fn request<'r>(&self) -> Request<'r> {
        let request = Request::new(self.method, self.url.clone()).header("Accept", self.accept);
        match &self.body {
            Some(body) => request.json(body.clone()),
            None => request,
        }
    }
}

impl Op for Call<'_, '_> {
    type Output = Response;

    /// The request, its credential header shown as `***`.
    fn plan(&self) -> Value {
        let mut plan = self.request().plan();
        let header = match self.client.instance.auth {
            Auth::Password { .. } => "Authorization",
            Auth::Token(_) => "x-api-key",
        };
        plan["headers"][header] = json!("***");
        plan
    }

    fn writes(&self) -> bool {
        !self.method.is_read()
    }

    fn perform(self, ctx: &Ctx) -> Result<Response> {
        let base = &self.client.instance.base_url;
        if !same_origin(base, &self.url) {
            bail!(
                "refusing to send the Control-M credential to {}; it goes only under {base}",
                self.url
            );
        }
        let request = self.request();
        match &self.client.instance.auth {
            Auth::Token(token) => request
                .header("x-api-key", self.client.key(token)?.expose())
                .perform(ctx),
            Auth::Password { .. } => {
                let mint = |fresh: bool| self.client.bearer(fresh);
                request.auth(&mint).perform(ctx)
            }
        }
    }
}

/// Control-M's refusals as the next step. Core already reads its
/// `{"errors": [{"message"}]}` into the failure's message.
// VERIFY(work): which status a job id that does not exist gets. The spec
// lists 404, but some versions answer 400 or 500 with a message; match that
// message here so it is exit 4 with the list's hint too.
pub(crate) fn refused(error: anyhow::Error, path: &str) -> anyhow::Error {
    let mut failure = match error.downcast::<Failure>() {
        Ok(failure) => failure,
        // No answer at all: the server is down, base_url points nowhere, or
        // its certificate is not one agent-cli trusts.
        Err(error) => {
            return Failure::new(Exit::Failed, format!("{error:#}"))
                .hint("is the Automation API up at base_url? `agent-cli doctor controlm` checks it")
                .into();
        }
    };
    match failure.status {
        Some(401) => {
            failure.hint = Some(
                "the Control-M credential was refused; `agent-cli doctor controlm` checks it"
                    .to_owned(),
            );
        }
        Some(403) => {
            failure.hint = Some(
                "the Control-M user behind this credential lacks this permission; ask the Control-M admins"
                    .to_owned(),
            );
        }
        Some(404) if path.starts_with("run/job/") => {
            failure.exit = Exit::NotFound;
            failure.hint = Some("agent-cli controlm job list --name NAME".to_owned());
        }
        Some(404) if path.starts_with("deploy/") => {
            failure.exit = Exit::NotFound;
            failure.hint = Some("agent-cli controlm definition list --name NAME".to_owned());
        }
        Some(404) => {
            failure.exit = Exit::Setup;
            failure.hint = Some(
                "base_url must be the Automation API endpoint, usually https://HOST:8443/automation-api; fix it under [[controlm.instance]]"
                    .to_owned(),
            );
        }
        _ => {}
    }
    failure.into()
}

// ---------- requests ----------

/// `path?key=value&…` with each value percent-encoded; empty values left out.
pub(crate) fn with_query(path: &str, pairs: &[(&str, String)]) -> String {
    let mut out = path.to_owned();
    for (key, value) in pairs.iter().filter(|(_, value)| !value.is_empty()) {
        out.push(if out.contains('?') { '&' } else { '?' });
        out.push_str(key);
        out.push('=');
        percent_encode(value, &mut out);
    }
    out
}

/// A time as the API takes it: `YYYYMMDDhhmmss` on the servers' clock.
// VERIFY(work): the format of fromTime and toTime in GET /run/jobs/status;
// the spec types them as plain strings.
pub(crate) fn compact(when: When, offset: UtcOffset) -> String {
    let at = when.0.to_offset(offset);
    format!(
        "{:04}{:02}{:02}{:02}{:02}{:02}",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
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

/// A time Control-M printed, as RFC 3339 UTC: `YYYYMMDDhhmmss` on the
/// servers' clock (`utc_offset`), or an ISO 8601 stamp; anything else as it
/// came.
// VERIFY(work): BMC's examples show startTime and endTime as
// `YYYYMMDDhhmmss`; check yours, and that they are on the servers' clock.
pub(crate) fn stamp(value: &Value, offset: UtcOffset) -> Option<String> {
    let raw = text(value)?;
    Some(match local_time(&raw) {
        Some(local) => utc_time(local.assume_offset(offset)),
        None => utc(&raw),
    })
}

fn local_time(raw: &str) -> Option<PrimitiveDateTime> {
    if raw.len() != 14 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let month = Month::try_from(raw[4..6].parse::<u8>().ok()?).ok()?;
    let date =
        Date::from_calendar_date(raw[..4].parse().ok()?, month, raw[6..8].parse().ok()?).ok()?;
    let time = Time::from_hms(
        raw[8..10].parse().ok()?,
        raw[10..12].parse().ok()?,
        raw[12..14].parse().ok()?,
    )
    .ok()?;
    Some(PrimitiveDateTime::new(date, time))
}

/// An order date (the plan day a job was ordered into) as `YYYY-MM-DD`, from
/// `YYMMDD` or `YYYYMMDD`; anything else as it came.
pub(crate) fn order_date(value: &Value) -> Option<String> {
    let raw = text(value)?;
    let digits = raw.bytes().all(|byte| byte.is_ascii_digit());
    Some(match raw.len() {
        6 if digits => format!("20{}-{}-{}", &raw[..2], &raw[2..4], &raw[4..]),
        8 if digits => format!("{}-{}-{}", &raw[..4], &raw[4..6], &raw[6..]),
        _ => raw,
    })
}

/// `[50 of 312; --limit N]` when there are more than were shown.
pub(crate) fn note_more(ctx: &Ctx, shown: usize, total: Option<usize>) {
    if let Some(total) = total.filter(|total| *total > shown) {
        ctx.note(format!("[{shown} of {total}; --limit N]"));
    }
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::testing::{API, CONFIG, PASSWORD_CONFIG, TOKEN, controlm_with, job};

    #[test]
    fn an_api_token_goes_as_x_api_key_and_only_under_base_url() {
        let (outcome, transport) = controlm_with(
            CONFIG,
            &["controlm", "job", "get", "ctm-prod:00a1b"],
            vec![Answer::json(&job(
                "ctm-prod:00a1b",
                "load_orders",
                "Ended OK",
            ))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = &transport.sent()[0];
        assert!(sent.url.starts_with(API), "{}", sent.url);
        assert_eq!(sent.authorization, None);
        assert!(
            sent.headers
                .iter()
                .any(|(name, value)| name == "x-api-key" && value == TOKEN),
            "{:?}",
            sent.headers
        );
        assert!(!same_origin(API, "https://ctm.contoso.example:8443/other"));
    }

    #[test]
    fn a_password_logs_in_once_and_sends_the_session_token_as_bearer() {
        let (outcome, transport) = controlm_with(
            PASSWORD_CONFIG,
            &["controlm", "job", "get", "ctm-prod:00a1b"],
            vec![
                Answer::json(
                    &json!({"username": "agent", "token": "session-1", "version": "9.0.21"}),
                ),
                Answer::json(&job("ctm-prod:00a1b", "load_orders", "Ended OK")),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert_eq!(sent[0].url, format!("{API}/session/login"));
        assert_eq!(sent[0].body.as_ref().unwrap()["username"], "agent");
        assert_eq!(sent[1].authorization.as_deref(), Some("Bearer session-1"));
        assert!(!outcome.stdout.contains("session-1"));
    }

    #[test]
    fn a_refused_password_is_exit_3_naming_doctor() {
        let (outcome, _) = controlm_with(
            PASSWORD_CONFIG,
            &["controlm", "job", "get", "ctm-prod:00a1b"],
            vec![Answer::status(
                401,
                json!({"errors": [{"message": "Invalid user name or password"}]}).to_string(),
            )],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(outcome.stderr.contains("Invalid user name or password"));
        assert!(outcome.stderr.contains("doctor controlm"));
    }

    #[test]
    fn times_turn_utc_from_the_servers_clock_and_order_dates_into_days() {
        let plus_two = UtcOffset::from_hms(2, 0, 0).unwrap();
        assert_eq!(
            stamp(&json!("20260929021500"), plus_two).as_deref(),
            Some("2026-09-29T00:15:00Z")
        );
        assert_eq!(
            stamp(&json!("2026-09-29T02:15:00+02:00"), plus_two).as_deref(),
            Some("2026-09-29T00:15:00Z")
        );
        assert_eq!(stamp(&json!(""), plus_two), None);
        assert_eq!(order_date(&json!("260929")).as_deref(), Some("2026-09-29"));
        assert_eq!(
            order_date(&json!("20260929")).as_deref(),
            Some("2026-09-29")
        );
        assert_eq!(
            parse_offset("-05:30"),
            UtcOffset::from_hms(-5, -30, 0).map_err(|_| String::new())
        );
        assert!(parse_offset("0200").is_err());
    }

    #[test]
    fn a_broken_instance_is_exit_3_naming_it() {
        let config = "[[controlm.instance]]\nname = \"prod\"\nbase_url = \"http://ctm.contoso.example\"\ntoken_env = \"T\"\n";
        let (outcome, transport) = controlm_with(
            config,
            &["controlm", "job", "get", "ctm-prod:00a1b"],
            vec![],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(outcome.stderr.contains("\"prod\""), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());
    }
}
