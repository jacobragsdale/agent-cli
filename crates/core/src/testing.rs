//! Helpers for domain tests: a recording transport, an in-process runner, and
//! the harnesses every domain runs its commands through.
//!
//! Public rather than `cfg(test)` because domain crates' tests use them; they
//! cost the release binary nothing it does not call.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::config::Config;
use crate::ctx::{Ctx, Globals, Setup, locked};
use crate::dispatch::run_with;
use crate::http::{Body, Method, Request, Response, Transport};
use crate::registry::{Domain, Effect};
use crate::search::quality;
use crate::secret::Secret;

/// Search must put the labeled command first this often...
pub const TOP1_GATE: f64 = 0.80;
/// ...and in the top five this often.
pub const TOP5_GATE: f64 = 0.95;

/// One recorded answer.
#[derive(Clone, Debug)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Answer {
    #[must_use]
    pub fn ok(body: impl Into<String>) -> Self {
        Self::status(200, body)
    }

    #[must_use]
    pub fn json(body: &Value) -> Self {
        Self::ok(body.to_string())
    }

    #[must_use]
    pub fn status(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

/// A request as the fake transport saw it.
#[derive(Clone, Debug)]
pub struct Sent {
    pub method: Method,
    pub url: String,
    pub body: Option<Value>,
    /// The minted `Authorization` value, exposed so a test can assert which
    /// token signed which call.
    pub authorization: Option<String>,
}

/// A transport over recorded answers, keeping every request it was handed.
/// Clones share state, so a test keeps one and hands the other to [`Setup`].
#[derive(Clone, Default)]
pub struct FakeTransport {
    answers: Arc<Mutex<VecDeque<Answer>>>,
    sent: Arc<Mutex<Vec<Sent>>>,
}

impl FakeTransport {
    #[must_use]
    pub fn answering(answers: impl IntoIterator<Item = Answer>) -> Self {
        Self {
            answers: Arc::new(Mutex::new(answers.into_iter().collect())),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn sent(&self) -> Vec<Sent> {
        locked(&self.sent).clone()
    }

    /// Answers not yet asked for; a test that ends with some left asked for
    /// fewer calls than it recorded.
    #[must_use]
    pub fn remaining(&self) -> usize {
        locked(&self.answers).len()
    }
}

impl Transport for FakeTransport {
    fn send(
        &self,
        request: &Request<'_>,
        authorization: Option<&Secret>,
        _: Duration,
    ) -> Result<Response> {
        let body = match &request.body {
            Body::None => None,
            Body::Json(document) => Some(document.clone()),
            Body::Form(fields) => Some(Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            )),
        };
        locked(&self.sent).push(Sent {
            method: request.method,
            url: request.url.clone(),
            body,
            authorization: authorization.map(|secret| secret.expose().to_owned()),
        });
        let answer = locked(&self.answers).pop_front().ok_or_else(|| {
            anyhow::anyhow!(
                "the fake transport ran out of answers at {} {}",
                request.method.wire(),
                request.url
            )
        })?;
        Ok(Response {
            status: answer.status,
            headers: answer.headers,
            body: answer.body,
            url: request.url.clone(),
        })
    }
}

impl Setup {
    /// No config, no cache, not read-only, an empty environment, nothing on
    /// stdin and stand-in `az` tokens (`token@<resource>`), over `transport`.
    #[must_use]
    pub fn fake(transport: impl Transport + 'static) -> Self {
        Self {
            config: Config::empty(),
            transport: Box::new(transport),
            read_only: false,
            cache_dir: None,
            env: Some(Vec::new()),
            token: Some("token".to_owned()),
            stdin: Some(String::new()),
        }
    }

    /// What was piped to the run's stdin (`Ctx::long_text` reads it for `-`).
    #[must_use]
    pub fn with_stdin(mut self, text: &str) -> Self {
        self.stdin = Some(text.to_owned());
        self
    }

    /// One environment variable the run sees (`Ctx::env`).
    #[must_use]
    pub fn with_env(mut self, name: &str, value: &str) -> Self {
        self.env
            .get_or_insert_with(Vec::new)
            .push((name.to_owned(), value.to_owned()));
        self
    }

    /// Every `az` token is `{token}@{resource}` (`{token}-fresh@…` when
    /// minted again after a 401); no `az` runs.
    #[must_use]
    pub fn with_token(mut self, token: &str) -> Self {
        self.token = Some(token.to_owned());
        self
    }

    /// The config file's text, as if it were at `config.toml`.
    #[must_use]
    pub fn with_config(mut self, toml: &str) -> Self {
        self.config = Config::parse("config.toml", Some(toml), Vec::new());
        self
    }

    #[must_use]
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }
}

/// A [`Ctx`] with default globals, for calling a handler directly.
#[must_use]
pub fn ctx(setup: Setup) -> Ctx {
    Ctx::new(Globals::default(), setup, "agent-cli")
}

/// What a run printed and how it exited.
#[derive(Debug)]
pub struct Outcome {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    /// stdout as JSON; panics with both streams when it is not.
    #[must_use]
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|error| panic!("stdout is not JSON ({error}): {self:?}"))
    }
}

/// Commands whose output is someone else's data, passed through as it is:
/// their timestamps are the database's, not ours to rewrite.
const PASSTHROUGH: &[&str] = &["sql query run", "sql query bench"];

/// Runs `argv` (without the program name) in process, stdout not a terminal.
///
/// # Panics
/// When the run broke a rule every command keeps, so every fixture test
/// checks them for free: each `agent-cli …` line printed on stderr (a hint,
/// a note) must parse against `domains`, and every timestamp on stdout must
/// be RFC 3339 in UTC, ending in `Z`.
#[must_use]
pub fn run(domains: &[Domain], argv: &[&str], setup: Setup) -> Outcome {
    let argv: Vec<String> = argv.iter().map(|arg| (*arg).to_owned()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run_with(domains, &argv, setup, &mut out, &mut err, false);
    let outcome = Outcome {
        code,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    };
    let problems = printed_command_problems(domains, &outcome.stderr);
    assert!(
        problems.is_empty(),
        "a printed command line does not parse: {problems:#?}\n{outcome:?}"
    );
    let path = argv.iter().take(3).cloned().collect::<Vec<_>>().join(" ");
    if !PASSTHROUGH.contains(&path.as_str())
        && let Ok(printed) = serde_json::from_str::<Value>(&outcome.stdout)
    {
        let local = non_utc_times(&printed);
        assert!(
            local.is_empty(),
            "printed times must be RFC 3339 in UTC, ending in Z: {local:?}\n{outcome:?}"
        );
    }
    outcome
}

/// Every string in `value` that is an RFC 3339 time not written in UTC.
#[must_use]
pub fn non_utc_times(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => {
            let is_time =
                time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                    .is_ok();
            if is_time && !text.ends_with('Z') {
                vec![text.clone()]
            } else {
                Vec::new()
            }
        }
        Value::Array(items) => items.iter().flat_map(non_utc_times).collect(),
        Value::Object(map) => map.values().flat_map(non_utc_times).collect(),
        _ => Vec::new(),
    }
}

/// Each `agent-cli …` command line in `text` that does not parse against
/// `domains`, with why. See [`crate::registry::printed_command_problem`].
#[must_use]
pub fn printed_command_problems(domains: &[Domain], text: &str) -> Vec<String> {
    crate::registry::printed_commands(text)
        .into_iter()
        .filter_map(|line| {
            crate::registry::printed_command_problem(domains, &line)
                .map(|problem| format!("`agent-cli {line}`: {problem}"))
        })
        .collect()
}

/// The argv of the command a `[next: agent-cli …]` note in `text` names:
/// what an agent following notes runs next.
#[must_use]
pub fn next_command(text: &str) -> Option<Vec<String>> {
    let (_, rest) = text.split_once("[next: ")?;
    let line = crate::registry::printed_commands(rest).into_iter().next()?;
    crate::dispatch::shell_words(&line).ok()
}

/// Runs `argv --dry-run` over `answers` (the reads before its first change)
/// and asserts it stopped at a planned change with no write verb reaching the
/// transport. Returns what it would have done. Every write command's tests
/// call this.
///
/// # Panics
/// When the command did not reach `ctx.write`, or a change was sent.
#[must_use]
pub fn assert_dry_run(domains: &[Domain], argv: &[&str], answers: Vec<Answer>) -> Vec<Value> {
    let transport = FakeTransport::answering(answers);
    let mut argv = argv.to_vec();
    argv.push("--dry-run");
    let outcome = run(domains, &argv, Setup::fake(transport.clone()));
    let writes: Vec<Sent> = transport
        .sent()
        .into_iter()
        .filter(|sent| !sent.method.is_read())
        .collect();
    assert!(
        writes.is_empty(),
        "a change reached the transport under --dry-run: {writes:?}"
    );
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let printed = outcome.json();
    assert_eq!(
        printed["dry_run"], true,
        "the command finished without reaching ctx.write: {outcome:?}"
    );
    printed["would"].as_array().cloned().unwrap_or_default()
}

/// Asserts that under `AGENT_CLI_READ_ONLY` every write, destructive and
/// reveal command is refused with exit 2 before anything is sent.
///
/// # Panics
/// On the first command that is not refused.
pub fn assert_read_only_refuses(domains: &[Domain]) {
    for command in domains.iter().flat_map(|domain| domain.commands) {
        if !matches!(
            command.effect,
            Effect::Write | Effect::Destructive | Effect::Reveal
        ) {
            continue;
        }
        let transport = FakeTransport::default();
        let argv: Vec<String> = crate::dispatch::shell_words(command.example).unwrap_or_default();
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let outcome = run(domains, &argv, Setup::fake(transport.clone()).read_only());
        assert_eq!(outcome.code, 2, "{}: {outcome:?}", command.path.join(" "));
        assert!(
            outcome.stderr.contains("AGENT_CLI_READ_ONLY"),
            "{outcome:?}"
        );
        assert!(transport.sent().is_empty(), "{}", command.path.join(" "));
    }
}

/// Runs the labeled queries in `labeled` (TOML `[[query]] text expect`) and
/// asserts search meets [`TOP1_GATE`] and [`TOP5_GATE`]. No queries passes.
///
/// # Panics
/// When the file is invalid, names an unknown command, or search falls short.
pub fn assert_search_quality(domains: &[Domain], labeled: &str) {
    let quality = quality(domains, labeled).unwrap_or_else(|error| panic!("{error:#}"));
    eprintln!(
        "search: top-1 {}/{} ({:.2}), top-5 {}/{} ({:.2})",
        quality.top1,
        quality.total,
        quality.top1_rate(),
        quality.top5,
        quality.total,
        quality.top5_rate()
    );
    for miss in &quality.misses {
        eprintln!("  miss {miss}");
    }
    assert!(
        quality.top1_rate() >= TOP1_GATE && quality.top5_rate() >= TOP5_GATE,
        "search is below the gates (top-1 {TOP1_GATE}, top-5 {TOP5_GATE}); misses above"
    );
}
