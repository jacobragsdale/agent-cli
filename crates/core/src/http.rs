//! HTTP: the request type handlers build, the transport seam tests replace, and
//! the retry policy every domain shares.
//!
//! Ported from az-tui's Azure transport. The policy lives here and nowhere
//! above it: mint a fresh token and retry once on a `401`, wait out one
//! throttle when the deadline allows it, never follow a redirect, and turn
//! every other refusal into the message the service wrote with an exit code
//! that says what to do about it.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use time::OffsetDateTime;

use crate::ctx::{Ctx, Op};
use crate::error::{Exit, Failure};
use crate::said::failure_message;
use crate::secret::Secret;
use crate::throttle::throttle_wait;

/// The statuses services shed load with.
const THROTTLED: [u16; 2] = [429, 503];

/// Long enough for a slow handshake, short enough that a host that never
/// answers (a firewall that drops) does not look like a hang.
const CONNECT: Duration = Duration::from_secs(10);
/// For a write whose answer did not come: the read that tells is the domain's.
pub const MAY_HAVE_LANDED: &str =
    "the change may have been made before the deadline: read it back before running it again";

/// A body larger than this is not an answer any command wants; Azure DevOps
/// keeps attachments up to 60 MB.
const BODY_LIMIT: u64 = 64 * 1024 * 1024;

/// The verbs a request can use. [`Method::Query`] is a `POST` that only reads
/// (WIQL, Resource Graph, a token exchange): it is the one POST
/// [`Ctx::read`] accepts, so a create cannot slip through the read path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    Get,
    Query,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Query | Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    #[must_use]
    pub const fn is_read(self) -> bool {
        matches!(self, Self::Get | Self::Query)
    }
}

#[derive(Clone, Debug, Default)]
pub enum Body {
    #[default]
    None,
    /// Sent as `application/json` unless the request sets its own
    /// `Content-Type` (Azure DevOps wants `application/json-patch+json`).
    Json(Value),
    Form(Vec<(String, String)>),
    /// A file's bytes (an attachment), sent as `application/octet-stream`
    /// unless the request sets its own `Content-Type`.
    Bytes(Vec<u8>),
}

/// What mints the `Authorization` header value: asked with `fresh = false`
/// first and `fresh = true` once after a `401`.
pub type Mint<'a> = &'a dyn Fn(bool) -> Result<Secret>;

/// One HTTP request. Send it with [`Ctx::read`] or [`Ctx::write`].
///
/// Check the URL with [`host_under`] before attaching a token to it: a token
/// is only ever sent to its own audience's hosts, `nextLink`s included.
pub struct Request<'a> {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Body,
    pub auth: Option<Mint<'a>>,
    /// A `3xx` is the answer rather than a failure: a form sign-in's `302`
    /// carries the session cookie in its `Set-Cookie`. Still never followed.
    pub keep_redirect: bool,
}

impl<'a> Request<'a> {
    #[must_use]
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: Body::None,
            auth: None,
            keep_redirect: false,
        }
    }

    #[must_use]
    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::Get, url)
    }

    /// A `POST` that only reads, such as a WIQL or Resource Graph query.
    #[must_use]
    pub fn query(url: impl Into<String>, body: Value) -> Self {
        Self::new(Method::Query, url).json(body)
    }

    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    #[must_use]
    pub fn json(mut self, body: Value) -> Self {
        self.body = Body::Json(body);
        self
    }

    #[must_use]
    pub fn form(mut self, fields: Vec<(String, String)>) -> Self {
        self.body = Body::Form(fields);
        self
    }

    #[must_use]
    pub fn bytes(mut self, bytes: Vec<u8>) -> Self {
        self.body = Body::Bytes(bytes);
        self
    }

    #[must_use]
    pub fn auth(mut self, mint: Mint<'a>) -> Self {
        self.auth = Some(mint);
        self
    }

    #[must_use]
    pub fn keep_redirect(mut self) -> Self {
        self.keep_redirect = true;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// The answer as sent when it is not UTF-8, which `body` cannot hold
    /// (it is then empty): an attachment, an image.
    pub bytes: Option<Vec<u8>>,
    /// Where it came from, filled in by core for error messages.
    pub url: String,
}

impl Response {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The answer as sent, text or not.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes.unwrap_or_else(|| self.body.into_bytes())
    }

    /// The body as JSON. An empty body is an error, never "no rows": read that
    /// way it would turn a failure into a confident empty answer.
    pub fn json(&self) -> Result<Value> {
        if self.body.trim().is_empty() {
            bail!("{} answered with an empty body", self.url);
        }
        serde_json::from_str(&self.body)
            .with_context(|| format!("{} answered with something other than JSON", self.url))
    }
}

/// How requests reach the network. One seam, so domain tests drive every
/// command with recorded answers (see [`crate::testing::FakeTransport`]).
pub trait Transport: Send + Sync {
    /// One attempt, no retries. `authorization` is already minted; `timeout`
    /// is what is left of the command's deadline.
    fn send(
        &self,
        request: &Request<'_>,
        authorization: Option<&Secret>,
        timeout: Duration,
    ) -> Result<Response>;
}

/// The real transport. Redirects are not followed: a hop would drop the
/// `Authorization` header, or worse carry it, and trade a status this code can
/// read for a sign-in page it cannot.
#[derive(Default)]
pub struct Https {
    /// Built on first use, so a command that never calls out never pays for
    /// the TLS setup.
    agent: OnceLock<ureq::Agent>,
}

impl Transport for Https {
    fn send(
        &self,
        request: &Request<'_>,
        authorization: Option<&Secret>,
        timeout: Duration,
    ) -> Result<Response> {
        let agent = self.agent.get_or_init(|| {
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .build()
                .into()
        });
        let url = request.url.as_str();
        let mut headers: Vec<(&str, &str)> = request
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        if let Some(authorization) = authorization {
            headers.push(("Authorization", authorization.expose()));
        }
        let typed = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
        match &request.body {
            Body::Json(_) if !typed => headers.push(("Content-Type", "application/json")),
            Body::Form(_) if !typed => {
                headers.push(("Content-Type", "application/x-www-form-urlencoded"));
            }
            Body::Bytes(_) if !typed => headers.push(("Content-Type", "application/octet-stream")),
            _ => {}
        }
        // `ureq` types its builders by whether a body follows, so the verb and
        // the body are chosen together.
        let sent = match request.method {
            Method::Get => prepare(agent.get(url), &headers, timeout).call(),
            Method::Delete => prepare(agent.delete(url), &headers, timeout).call(),
            Method::Query | Method::Post => {
                send_body(prepare(agent.post(url), &headers, timeout), &request.body)
            }
            Method::Put => send_body(prepare(agent.put(url), &headers, timeout), &request.body),
            Method::Patch => send_body(prepare(agent.patch(url), &headers, timeout), &request.body),
        };
        let mut response = sent.map_err(|error| match error {
            ureq::Error::Timeout(ureq::Timeout::Connect) if timeout > CONNECT => {
                anyhow::Error::new(Failure::new(
                    Exit::Failed,
                    format!(
                        "{} {url} failed: no connection within {}s",
                        request.method.wire(),
                        CONNECT.as_secs()
                    ),
                ))
            }
            ureq::Error::Timeout(_) => {
                let late = Failure::timed_out(format!(
                    "{} {url} did not answer before the deadline",
                    request.method.wire()
                ));
                // A write that was sent may have been made: unanswered is not undone.
                let late = if request.method.is_read() {
                    late
                } else {
                    late.hint(MAY_HAVE_LANDED)
                };
                anyhow::Error::new(late).context(error)
            }
            other => {
                anyhow::Error::new(other).context(format!("{} {url} failed", request.method.wire()))
            }
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
            })
            .collect();
        let read = response
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_vec()
            .map_err(|error| match error {
                ureq::Error::Timeout(_) => anyhow::Error::new(Failure::timed_out(format!(
                    "{url} did not finish its answer before the deadline"
                ))),
                ureq::Error::BodyExceedsLimit(limit) => anyhow::Error::new(Failure::new(
                    Exit::Failed,
                    format!(
                        "{url} answered more than {} MiB, the most agent-cli reads",
                        limit / 1024 / 1024
                    ),
                )),
                other => anyhow::Error::new(other)
                    .context(format!("failed to read the answer from {url}")),
            })?;
        let (body, bytes) = match String::from_utf8(read) {
            Ok(body) => (body, None),
            Err(binary) => (String::new(), Some(binary.into_bytes())),
        };
        Ok(Response {
            status,
            headers,
            body,
            bytes,
            url: url.to_owned(),
        })
    }
}

fn prepare<B>(
    mut builder: ureq::RequestBuilder<B>,
    headers: &[(&str, &str)],
    timeout: Duration,
) -> ureq::RequestBuilder<B> {
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder
        .config()
        .timeout_global(Some(timeout))
        .timeout_connect(Some(timeout.min(CONNECT)))
        .build()
}

fn send_body(
    builder: ureq::RequestBuilder<ureq::typestate::WithBody>,
    body: &Body,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    match body {
        Body::None => builder.send_empty(),
        Body::Json(document) => builder.send(document.to_string()),
        Body::Form(fields) => builder.send(form_encode(fields)),
        Body::Bytes(bytes) => builder.send(&bytes[..]),
    }
}

impl Op for Request<'_> {
    type Output = Response;

    fn plan(&self) -> Value {
        let mut plan = json!({"method": self.method.wire(), "url": self.url});
        if !self.headers.is_empty() {
            let headers: Map<String, Value> = self
                .headers
                .iter()
                .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                .collect();
            plan["headers"] = Value::Object(headers);
        }
        match &self.body {
            Body::None => {}
            Body::Json(document) => plan["body"] = document.clone(),
            Body::Form(fields) => {
                let fields: Map<String, Value> = fields
                    .iter()
                    .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                    .collect();
                plan["form"] = Value::Object(fields);
            }
            Body::Bytes(bytes) => plan["bytes"] = json!(bytes.len()),
        }
        plan
    }

    fn writes(&self) -> bool {
        !self.method.is_read()
    }

    /// One call, with the one retry each of two refusals is worth: a spent
    /// token gets a fresh one, and a throttle is waited out when the wait fits
    /// inside the deadline. A wait that does not fit fails now with exit 124
    /// rather than sleeping into the same failure.
    fn perform(self, ctx: &Ctx) -> Result<Response> {
        let mut token = self.auth.map(|mint| mint(false)).transpose()?;
        let (mut reminted, mut waited) = (false, false);
        loop {
            let mut response = ctx
                .transport()
                .send(&self, token.as_ref(), ctx.remaining()?)?;
            response.url.clone_from(&self.url);
            if response.status == 401
                && !reminted
                && let Some(mint) = self.auth
            {
                reminted = true;
                token = Some(mint(true)?);
                continue;
            }
            // A 429 refused the call unread; a 503 may come after a write
            // was made, so only a read is sent again after one.
            let retry = response.status == 429 || self.method.is_read();
            if THROTTLED.contains(&response.status) && retry && !waited {
                waited = true;
                let wait = throttle_wait(&response, OffsetDateTime::now_utc());
                let left = ctx.remaining()?;
                if wait >= left {
                    return Err(Failure::timed_out(format!(
                        "{} {} is throttled and asks for {}s; the deadline leaves {}s",
                        self.method.wire(),
                        self.url,
                        wait.as_secs(),
                        left.as_secs()
                    ))
                    .hint("run it again later, or with a larger --timeout")
                    .into());
                }
                std::thread::sleep(wait);
                continue;
            }
            if self.keep_redirect && (300..400).contains(&response.status) {
                return Ok(response);
            }
            return checked(self.method, response);
        }
    }
}

/// A `2xx` as it is; anything else as a [`Failure`] whose exit code says what
/// kind of problem it is.
fn checked(method: Method, response: Response) -> Result<Response> {
    let status = response.status;
    if (200..300).contains(&status) {
        return Ok(response);
    }
    let exit = match status {
        400 | 422 => Exit::Usage,
        401 => Exit::Setup,
        404 => Exit::NotFound,
        409 | 412 => Exit::Conflict,
        _ => Exit::Failed,
    };
    let reason = match response.header("Location") {
        Some(location) if (300..400).contains(&status) => {
            format!("a redirect to {location}, which is not followed")
        }
        _ => failure_message(&response.body),
    };
    let mut failure = Failure::new(
        exit,
        format!(
            "{} {} answered {status}: {reason}",
            method.wire(),
            response.url
        ),
    );
    failure.status = Some(status);
    Err(match status {
        401 => failure.hint("the credential was refused: sign in again or check its scopes; `agent-cli doctor` shows what is set up"),
        503 if !method.is_read() => failure.hint(
            "the service may have made the change before it failed: check before running it again",
        ),
        429 | 503 => failure.hint("the service is still throttling; run it again later"),
        _ => failure,
    }
    .into())
}

/// True when `url` is `https://` to a plain DNS name under `suffix`.
///
/// A suffix with a leading dot (`.vault.azure.net`) needs a name under it; one
/// without (`dev.azure.com`) also matches itself. Anything unusual in the host
/// — a port, a user, a backslash a URL parser reads as a slash — is refused
/// rather than reasoned about. Callers check this before attaching a token.
#[must_use]
pub fn host_under(url: &str, suffix: &str) -> bool {
    let Some(host) = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
    else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let suffix = suffix.to_ascii_lowercase();
    let plain = host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
    let under = if suffix.starts_with('.') {
        host.len() > suffix.len() && host.ends_with(&suffix)
    } else {
        host == suffix || host.ends_with(&format!(".{suffix}"))
    };
    plain && under && !suffix.is_empty()
}

/// Standard base64 with padding: the `Basic` scheme's `user:password`.
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
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

/// `application/x-www-form-urlencoded`, which is also a query string.
#[must_use]
pub fn form_encode(pairs: &[(String, String)]) -> String {
    let mut encoded = String::new();
    for (key, value) in pairs {
        if !encoded.is_empty() {
            encoded.push('&');
        }
        percent_encode(key, &mut encoded);
        encoded.push('=');
        percent_encode(value, &mut encoded);
    }
    encoded
}

/// Everything outside RFC 3986's unreserved set escaped, and a space as `+`.
pub fn percent_encode(raw: &str, out: &mut String) {
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
}

/// How long is left before `deadline`, or a timeout failure when nothing is.
pub(crate) fn left_before(deadline: Instant) -> Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(Failure::timed_out("the command ran out of time")
            .hint("run it again with a larger --timeout")
            .into());
    }
    Ok(left)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::ctx::{Globals, Setup};
    use crate::error::describe;
    use crate::testing::{Answer, FakeTransport, ctx};

    fn fake(answers: Vec<Answer>) -> (Ctx, FakeTransport) {
        let transport = FakeTransport::answering(answers);
        (ctx(Setup::fake(transport.clone())), transport)
    }

    #[test]
    fn only_https_to_a_plain_name_under_the_suffix_is_that_host() {
        assert!(host_under(
            "https://kv-prod.vault.azure.net/secrets?x=1",
            ".vault.azure.net"
        ));
        assert!(host_under("https://KV.Vault.Azure.Net", ".vault.azure.net"));
        assert!(host_under(
            "https://dev.azure.com/org/_apis",
            "dev.azure.com"
        ));
        assert!(host_under("https://x.dev.azure.com/", "dev.azure.com"));
        for url in [
            "https://evil.example/",
            "https://.vault.azure.net/",
            "https://vault.azure.net/",
            "https://evil.example\\.vault.azure.net/",
            "https://x.vault.azure.net@evil.example/",
            "https://kv.vault.azure.net:8443/",
            "https://evil.vault.azure.net.example/",
            "http://kv.vault.azure.net/",
            "https://notdev.azure.com/",
            "kv.vault.azure.net",
        ] {
            assert!(
                !host_under(url, ".vault.azure.net") && !host_under(url, "dev.azure.com"),
                "{url}"
            );
        }
    }

    #[test]
    fn a_form_is_encoded_as_a_query_string() {
        assert_eq!(
            form_encode(&[("a b".into(), "x/y&z".into())]),
            "a+b=x%2Fy%26z"
        );
        for (raw, encoded) in [
            ("", ""),
            ("a", "YQ=="),
            ("ab", "YWI="),
            ("admin:admin", "YWRtaW46YWRtaW4="),
        ] {
            assert_eq!(base64(raw.as_bytes()), encoded);
        }
    }

    #[test]
    fn a_spent_token_is_minted_once_more_and_the_call_retried() {
        let (ctx, transport) = fake(vec![
            Answer::status(401, "{}"),
            Answer::json(&serde_json::json!({"ok": 1})),
        ]);
        let mints = std::sync::Mutex::new(Vec::new());
        let mint = |fresh: bool| {
            mints.lock().unwrap().push(fresh);
            Ok(Secret::new(format!("Bearer minted-token-{fresh}")))
        };
        let response = ctx
            .read(Request::get("https://h.example/x").auth(&mint))
            .unwrap();
        assert_eq!(response.json().unwrap()["ok"], 1);
        assert_eq!(*mints.lock().unwrap(), [false, true]);
        let sent = transport.sent();
        assert_eq!(
            sent[0].authorization.as_deref(),
            Some("Bearer minted-token-false")
        );
        assert_eq!(
            sent[1].authorization.as_deref(),
            Some("Bearer minted-token-true")
        );
    }

    #[test]
    fn a_second_401_is_needs_setup_and_statuses_map_to_exit_codes() {
        let mint = |_: bool| Ok(Secret::new("Bearer same-old-token"));
        let (ctx, _) = fake(vec![
            Answer::status(401, "{}"),
            Answer::status(401, r#"{"message":"expired"}"#),
        ]);
        let error = ctx
            .read(Request::get("https://h.example/x").auth(&mint))
            .unwrap_err();
        let (exit, message, hint) = describe(&error);
        assert_eq!(exit, Exit::Setup);
        assert_eq!(crate::error::status_of(&error), Some(401));
        assert_eq!(message, "GET https://h.example/x answered 401: expired");
        assert!(hint.unwrap().contains("sign in"));

        for (status, exit) in [
            (404, Exit::NotFound),
            (409, Exit::Conflict),
            (412, Exit::Conflict),
            (400, Exit::Usage),
            (500, Exit::Failed),
        ] {
            let (ctx, _) = fake(vec![Answer::status(status, "{}")]);
            let error = ctx.read(Request::get("https://h.example/x")).unwrap_err();
            assert_eq!(describe(&error).0, exit, "{status}");
        }
        let (ctx, _) = fake(vec![
            Answer::status(302, "").with_header("Location", "https://login.example/"),
        ]);
        let error = ctx.read(Request::get("https://h.example/x")).unwrap_err();
        assert!(
            describe(&error)
                .1
                .contains("redirect to https://login.example/, which is not followed")
        );
        let (ctx, _) = fake(vec![
            Answer::status(302, "").with_header("Set-Cookie", "session=s1; Path=/"),
        ]);
        let kept = ctx
            .read(Request::get("https://h.example/login/").keep_redirect())
            .unwrap();
        assert_eq!(
            (kept.status, kept.header("set-cookie")),
            (302, Some("session=s1; Path=/"))
        );
    }

    #[test]
    fn a_throttle_is_waited_out_once_when_the_deadline_allows_it() {
        let (ctx, transport) = fake(vec![
            Answer::status(429, "{}").with_header("Retry-After", "1"),
            Answer::json(&serde_json::json!([1])),
        ]);
        let started = Instant::now();
        ctx.read(Request::get("https://h.example/x")).unwrap();
        assert!(started.elapsed() >= Duration::from_secs(1));
        assert_eq!(transport.sent().len(), 2);
    }

    #[test]
    fn a_throttle_longer_than_the_deadline_fails_now_with_124() {
        let transport = FakeTransport::answering(vec![Answer::status(503, "{}")]);
        let globals = Globals {
            timeout: Some(Duration::from_secs(5)),
            ..Globals::default()
        };
        let ctx = Ctx::new(globals, Setup::fake(transport.clone()), "agent-cli");
        let started = Instant::now();
        let error = ctx.read(Request::get("https://h.example/x")).unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "no sleep into the same failure"
        );
        let (exit, message, _) = describe(&error);
        assert_eq!(exit, Exit::TimedOut);
        assert!(message.contains("asks for 30s"), "{message}");
    }

    #[test]
    fn the_read_path_refuses_a_write_verb_and_the_write_path_sends_it() {
        let (ctx, transport) = fake(vec![Answer::ok("{}")]);
        let error = ctx
            .read(Request::new(Method::Delete, "https://h.example/x"))
            .unwrap_err();
        assert!(error.to_string().contains("ctx.write"), "{error}");
        assert!(transport.sent().is_empty());
        ctx.write(
            crate::Effect::Write,
            Request::new(Method::Post, "https://h.example/x").json(serde_json::json!({})),
        )
        .unwrap();
        assert_eq!(transport.sent()[0].method, Method::Post);
    }

    #[test]
    fn an_empty_or_non_json_body_is_an_error_naming_the_url() {
        let empty = Response {
            url: "https://h.example/x".into(),
            ..Response::default()
        };
        assert_eq!(
            empty.json().unwrap_err().to_string(),
            "https://h.example/x answered with an empty body"
        );
        let html = Response {
            body: "<html>".into(),
            ..empty
        };
        assert!(
            html.json()
                .unwrap_err()
                .to_string()
                .contains("other than JSON")
        );
    }

    /// One HTTP exchange on a loopback socket: the request as received, and
    /// `answer` sent back.
    fn serve_once(answer: &'static str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut received = Vec::new();
            let mut buffer = [0; 4096];
            while !received.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                received.extend_from_slice(&buffer[..read]);
            }
            if !answer.is_empty() {
                socket.write_all(answer.as_bytes()).unwrap();
            } else {
                std::thread::sleep(Duration::from_secs(2));
            }
            String::from_utf8_lossy(&received).into_owned()
        });
        (url, server)
    }

    #[test]
    fn the_real_transport_sends_the_token_and_never_follows_a_redirect() {
        let (url, server) = serve_once(
            "HTTP/1.1 302 Found\r\nLocation: http://elsewhere.example/\r\nContent-Length: 0\r\n\r\n",
        );
        let token = Secret::new("Bearer loopback-token-1");
        let request = Request::get(&url).header("Accept", "application/json");
        let response = Https::default()
            .send(&request, Some(&token), Duration::from_secs(5))
            .unwrap();
        assert_eq!(response.status, 302);
        assert_eq!(
            response.header("location"),
            Some("http://elsewhere.example/")
        );
        let received = server.join().unwrap().to_ascii_lowercase();
        assert!(received.starts_with("get /x http/1.1\r\n"), "{received}");
        assert!(
            received.contains("authorization: bearer loopback-token-1\r\n"),
            "{received}"
        );
        assert!(
            received.contains("accept: application/json\r\n"),
            "{received}"
        );
    }

    #[test]
    fn the_real_transport_times_out_with_124() {
        let (url, _server) = serve_once("");
        let started = Instant::now();
        let error = Https::default()
            .send(&Request::get(&url), None, Duration::from_millis(300))
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(describe(&error).0, Exit::TimedOut, "{error:#}");
        assert_eq!(
            describe(&error).2,
            None,
            "a read that timed out changed nothing"
        );

        // A write that went out unanswered may have been made.
        let (url, _server) = serve_once("");
        let request = Request::new(Method::Patch, &url).json(serde_json::json!({}));
        let error = Https::default()
            .send(&request, None, Duration::from_millis(300))
            .unwrap_err();
        assert_eq!(describe(&error).0, Exit::TimedOut, "{error:#}");
        assert_eq!(describe(&error).2.as_deref(), Some(MAY_HAVE_LANDED));
    }
}
