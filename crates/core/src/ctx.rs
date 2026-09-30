//! What a handler is handed: the globals, the deadline, config, cache, and the
//! two doors to the outside world.
//!
//! Every effect a command has goes through [`Ctx::read`] or [`Ctx::write`],
//! whether it is an HTTP request, a child process or a SQL batch. That makes
//! `write` the one place `--dry-run`, `AGENT_CLI_READ_ONLY` and `--yes` are
//! enforced, so a handler cannot forget them.

use std::collections::HashMap;
use std::path::PathBuf;
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
    /// The command line as typed, for "run it again with --yes".
    command_line: String,
    plans: Mutex<Vec<Value>>,
    notes: Mutex<Vec<String>>,
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
            command_line: command_line.into(),
            plans: Mutex::new(Vec::new()),
            notes: Mutex::new(Vec::new()),
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

    pub(crate) fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut *locked(&self.notes))
    }
}

/// A poisoned lock still holds a usable value; a panicked thread's half-done
/// push is no reason to lose the rest.
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
