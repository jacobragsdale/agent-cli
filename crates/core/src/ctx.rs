//! What a handler is handed: the globals, the deadline, config, cache, and the
//! two doors to the outside world.
//!
//! Every effect a command has goes through [`Ctx::read`] or [`Ctx::write`],
//! whether it is an HTTP request, a child process or a SQL batch. That makes
//! `write` the one place `--dry-run`, `AGENT_CLI_READ_ONLY` and `--yes` are
//! enforced, so a handler cannot forget them.

use std::collections::HashMap;
use std::io::{IsTerminal as _, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::cache::Cache;
use crate::config::Config;
use crate::error::Failure;
use crate::http::{Https, Transport, left_before};
use crate::registry::Effect;
use crate::secret::{Secret, redact_value};

/// The flags every command accepts, anywhere on the line.
#[derive(Clone, Debug, Default)]
pub struct Globals {
    pub fields: Option<String>,
    pub raw: bool,
    pub dry_run: bool,
    pub yes: bool,
    pub reveal: bool,
    pub no_cache: bool,
    pub help: bool,
    /// `--timeout`; `None` is the command's own default, else [`DEFAULT_TIMEOUT`].
    pub timeout: Option<Duration>,
    pub output: Option<PathBuf>,
}

/// Claude Code's shell tool gives up at two minutes; this leaves room for the
/// agent to read the error and try again.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Everything a run needs from its surroundings. [`Setup::from_env`] is the
/// real one; tests build one around a fake transport and a config string.
pub struct Setup {
    pub config: Config,
    pub transport: Box<dyn Transport>,
    pub read_only: bool,
    /// `None` turns the cache off.
    pub cache_dir: Option<PathBuf>,
    /// The environment [`Ctx::env`] reads: `None` is the process's own; tests
    /// pass only what they set, since edition 2024 makes `set_var` unsafe.
    pub env: Option<Vec<(String, String)>>,
    /// A stand-in for every `az` token, so tests never start `az`: see
    /// [`Ctx::az_token`].
    pub token: Option<String>,
    /// What [`Ctx::long_text`] reads for `-`: `None` is the process's stdin;
    /// tests pass what was piped in, and empty is nothing.
    pub stdin: Option<String>,
}

impl Setup {
    #[must_use]
    pub fn from_env() -> Self {
        let read_only = std::env::var("AGENT_CLI_READ_ONLY")
            .is_ok_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
        let setup = Self {
            config: Config::load(),
            transport: Box::new(Https::default()),
            read_only,
            cache_dir: crate::cache::default_dir(),
            env: None,
            token: None,
            stdin: None,
        };
        #[cfg(feature = "fixtures")]
        let setup = crate::replay::from_env(setup);
        setup
    }
}

/// Something a command does to the outside world: an HTTP request, a child
/// process, a SQL batch. Performed only through [`Ctx::read`] or
/// [`Ctx::write`].
pub trait Op {
    type Output;

    /// What `--dry-run` prints for it. Core redacts it.
    fn plan(&self) -> Value;

    /// True when the op is a change by its nature (a `PUT`, an `UPDATE`), so
    /// [`Ctx::read`] refuses it. An op that cannot tell, like a child
    /// process, keeps the default and the handler picks the door.
    fn writes(&self) -> bool {
        false
    }

    fn perform(self, ctx: &Ctx) -> Result<Self::Output>;
}

/// The marker [`Ctx::write`] returns under `--dry-run`. Core sees the recorded
/// plan and prints it, whatever the handler did with the error.
#[derive(Debug)]
pub(crate) struct DryRun;

impl std::fmt::Display for DryRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("stopped before the first change (--dry-run)")
    }
}

impl std::error::Error for DryRun {}

pub struct Ctx {
    globals: Globals,
    deadline: Instant,
    config: Config,
    cache: Cache,
    transport: Box<dyn Transport>,
    read_only: bool,
    env: Option<Vec<(String, String)>>,
    pub(crate) token: Option<String>,
    stdin: Mutex<Option<String>>,
    /// The command line as typed, for "run it again with --yes".
    command_line: String,
    plans: Mutex<Vec<Value>>,
    notes: Mutex<Vec<String>>,
    /// The handler wrote `--output` itself ([`Ctx::save`]).
    saved: Mutex<bool>,
    pub(crate) tokens: Mutex<HashMap<String, Secret>>,
}

impl Ctx {
    #[must_use]
    pub fn new(globals: Globals, setup: Setup, command_line: impl Into<String>) -> Self {
        let cache_dir = setup.cache_dir.filter(|_| !globals.no_cache);
        Self {
            deadline: Instant::now() + globals.timeout.unwrap_or(DEFAULT_TIMEOUT),
            globals,
            config: setup.config,
            cache: Cache::new(cache_dir),
            transport: setup.transport,
            read_only: setup.read_only,
            env: setup.env,
            token: setup.token,
            stdin: Mutex::new(setup.stdin),
            command_line: command_line.into(),
            plans: Mutex::new(Vec::new()),
            notes: Mutex::new(Vec::new()),
            saved: Mutex::new(false),
            tokens: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn globals(&self) -> &Globals {
        &self.globals
    }

    #[must_use]
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// What is left of `--timeout`, or exit 124 when nothing is.
    pub fn remaining(&self) -> Result<Duration> {
        left_before(self.deadline)
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// This domain's `[name]` section, with `AGENT_CLI_<NAME>_<KEY>` applied.
    pub fn section<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        self.config.section(name)
    }

    #[must_use]
    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    /// An environment variable, set and not blank. Every read of one goes
    /// through here, so tests set it with `Setup::with_env`.
    #[must_use]
    pub fn env(&self, name: &str) -> Option<String> {
        let value = match &self.env {
            None => std::env::var(name).ok(),
            Some(pairs) => pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone()),
        };
        value.filter(|value| !value.trim().is_empty())
    }

    /// Long text as every command takes it: `value` as typed, stdin when it
    /// is `-`, or the file `--{name}-file` names (`None` when neither was
    /// given, and both is exit 2). `name` is the argument's: `description`,
    /// `text`. Text from stdin or a file loses its `\r`s and trailing
    /// whitespace, must hold something, and is at most `limit` bytes when
    /// there is one: read one byte past it and no further, so a log of any
    /// size is refused without first being held in memory.
    pub fn long_text(
        &self,
        name: &str,
        value: Option<&str>,
        file: Option<&Path>,
        limit: Option<usize>,
    ) -> Result<Option<LongText>> {
        let cap = limit.map_or(u64::MAX, |limit| limit as u64 + 1);
        let what = name.replace('-', " ");
        let mut raw = String::new();
        let (from, piped) = match (value, file) {
            (Some(_), Some(path)) => {
                return Err(Failure::usage(format!(
                    "the {what} came both as a value and as the file {}",
                    path.display()
                ))
                .hint(format!("pass the {what} or --{name}-file, not both"))
                .into());
            }
            (None, None) => return Ok(None),
            (Some(text), None) if text != "-" => {
                return Ok(Some(LongText {
                    text: text.to_owned(),
                    piped: false,
                }));
            }
            (Some(_), None) => {
                match locked(&self.stdin).take() {
                    Some(given) => raw = given,
                    None => {
                        let stdin = std::io::stdin();
                        if stdin.is_terminal() {
                            return Err(Failure::usage(format!(
                                "`-` reads the {what} from stdin, and nothing is piped in"
                            ))
                            .hint(format!("pipe it in, or pass --{name}-file PATH"))
                            .into());
                        }
                        stdin
                            .lock()
                            .take(cap)
                            .read_to_string(&mut raw)
                            .map_err(|error| {
                                Failure::usage(format!(
                                    "cannot read the {what} from stdin: {error}"
                                ))
                            })?;
                    }
                }
                ("stdin".to_owned(), true)
            }
            (None, Some(path)) => {
                std::fs::File::open(path)
                    .and_then(|file| file.take(cap).read_to_string(&mut raw))
                    .map_err(|error| {
                        Failure::usage(format!(
                            "cannot read the {what} from {}: {error}",
                            path.display()
                        ))
                    })?;
                (path.display().to_string(), false)
            }
        };
        if let Some(limit) = limit
            && raw.len() > limit
        {
            return Err(Failure::usage(format!(
                "the {what} from {from} is more than {} KiB",
                limit / 1024
            ))
            .hint("cut it to the part worth reading first, such as with `tail -200`")
            .into());
        }
        let text = raw.replace("\r\n", "\n").trim_end().to_owned();
        if text.trim().is_empty() {
            return Err(Failure::usage(format!("the {what} from {from} is empty")).into());
        }
        Ok(Some(LongText { text, piped }))
    }

    /// A line for stderr after the output, such as `[50 of 312; --limit N]`.
    /// stdout carries data only.
    pub fn note(&self, text: impl Into<String>) {
        locked(&self.notes).push(text.into());
    }

    /// Performs an op that changes nothing.
    pub fn read<O: Op>(&self, op: O) -> Result<O::Output> {
        if op.writes() {
            bail!(
                "a change was sent through ctx.read: {}; send it with ctx.write",
                redact_value(op.plan())
            );
        }
        op.perform(self)
    }

    /// Performs an op that changes something, after the checks every change
    /// gets: refused under `AGENT_CLI_READ_ONLY`, recorded and stopped under
    /// `--dry-run` (reads before it have run), and refused without `--yes`
    /// when `effect` is [`Effect::Destructive`].
    pub fn write<O: Op>(&self, effect: Effect, op: O) -> Result<O::Output> {
        if self.read_only {
            return Err(
                Failure::usage("AGENT_CLI_READ_ONLY is set, so this change was refused")
                    .hint("unset AGENT_CLI_READ_ONLY to allow changes")
                    .into(),
            );
        }
        if self.globals.dry_run {
            locked(&self.plans).push(redact_value(op.plan()));
            return Err(DryRun.into());
        }
        if effect == Effect::Destructive && !self.globals.yes {
            return Err(
                Failure::usage("this change is destructive; confirm it with --yes")
                    .hint(format!(
                        "{} --yes   (or --dry-run to see it first)",
                        self.command_line
                    ))
                    .into(),
            );
        }
        op.perform(self)
    }

    pub(crate) fn transport(&self) -> &dyn Transport {
        self.transport.as_ref()
    }

    pub(crate) fn take_plans(&self) -> Vec<Value> {
        std::mem::take(&mut *locked(&self.plans))
    }

    /// Writes `bytes` to `--output FILE` (mode 0600) and returns the path;
    /// `None` without `--output`. For an answer that is a file rather than
    /// JSON (an attachment): the command's row then prints to stdout instead
    /// of being saved over the file.
    pub fn save(&self, bytes: &[u8]) -> Result<Option<PathBuf>> {
        let Some(path) = &self.globals.output else {
            return Ok(None);
        };
        crate::output::write_private(path, bytes)?;
        *locked(&self.saved) = true;
        Ok(Some(path.clone()))
    }

    /// The globals output is printed under: `--output` is spent once
    /// [`Ctx::save`] wrote it.
    pub(crate) fn printing(&self) -> Globals {
        let mut globals = self.globals.clone();
        if *locked(&self.saved) {
            globals.output = None;
        }
        globals
    }

    pub(crate) fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut *locked(&self.notes))
    }
}

/// What [`Ctx::long_text`] read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LongText {
    pub text: String,
    /// It came from stdin: program output, as `ado … comment -` posts it.
    pub piped: bool,
}

/// A poisoned lock still holds a usable value; a panicked thread's half-done
/// push is no reason to lose the rest.
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::testing::{FakeTransport, ctx};
    use crate::{Exit, Failure, Setup};

    fn exit(error: &anyhow::Error) -> Exit {
        error
            .downcast_ref::<Failure>()
            .map(|failure| failure.exit)
            .unwrap()
    }

    #[test]
    fn long_text_is_typed_piped_with_a_dash_or_read_from_a_file_and_capped() {
        let piped = |text: &str| ctx(Setup::fake(FakeTransport::default()).with_stdin(text));
        let typed = piped("");
        let read =
            |ctx: &super::Ctx, value, file, limit| ctx.long_text("description", value, file, limit);
        assert_eq!(read(&typed, None, None, None).unwrap(), None);
        let got = read(&typed, Some("  kept as typed \n"), None, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            (got.text.as_str(), got.piped),
            ("  kept as typed \n", false)
        );

        let got = read(&piped("# Why\r\n\r\nBecause\r\n\n"), Some("-"), None, None)
            .unwrap()
            .unwrap();
        assert_eq!((got.text.as_str(), got.piped), ("# Why\n\nBecause", true));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("why.md");
        std::fs::write(&path, "**Why**\n").unwrap();
        let got = read(&typed, None, Some(&path), Some(64)).unwrap().unwrap();
        assert_eq!((got.text.as_str(), got.piped), ("**Why**", false));
        let big = dir.path().join("big.md");
        std::fs::write(&big, "x".repeat(1025)).unwrap();

        for (ctx, value, file, limit, said) in [
            (
                piped(""),
                Some("-"),
                None,
                None,
                "the description from stdin is empty",
            ),
            (
                piped(&"x".repeat(1025)),
                Some("-"),
                None,
                Some(1024),
                "the description from stdin is more than 1 KiB",
            ),
            (
                piped(""),
                None,
                Some(big.as_path()),
                Some(1024),
                "big.md is more than 1 KiB",
            ),
            (
                piped(""),
                Some("x"),
                Some(path.as_path()),
                None,
                "both as a value and as the file",
            ),
            (
                piped(""),
                None,
                Some(Path::new("/nonexistent/why.md")),
                None,
                "cannot read the description from /nonexistent/why.md",
            ),
        ] {
            let error = read(&ctx, value, file, limit).unwrap_err();
            assert_eq!(exit(&error), Exit::Usage, "{error:#}");
            assert!(error.to_string().contains(said), "{error:#}");
        }
    }
}
