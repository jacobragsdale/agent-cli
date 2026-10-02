//! The runtime behind `agent-cli`: registry, dispatch, discovery, search,
//! output, safety, config, cache, child processes and HTTP.
//!
//! A domain crate exports `pub const DOMAIN: Domain` listing its commands,
//! each made with [`command!`]. Everything an agent relies on across domains
//! — the three-word paths, search, help, `--fields`, the output guard,
//! `--dry-run`, read-only mode, exit codes, redaction — lives here, so a new
//! command gets all of it by being registered.

mod az;
mod cache;
mod config;
mod credential;
mod ctx;
mod discover;
mod dispatch;
mod error;
mod http;
mod output;
mod process;
mod registry;
#[cfg(feature = "fixtures")]
mod replay;
mod search;
mod secret;
pub mod testing;
mod throttle;
mod when;

pub use anyhow;
pub use clap;
pub use schemars;
pub use serde_json;

pub use cache::Cache;
pub use config::{Config, pick};
pub use credential::Credential;
pub use ctx::{Ctx, DEFAULT_TIMEOUT, Globals, LongText, Op, Setup};
pub use discover::command_help;
pub use dispatch::{run, run_with};
pub use error::{Exit, Failure, status_of};
pub use http::{
    Body, Https, Method, Mint, Request, Response, Transport, failure_message, form_encode,
    host_under, percent_encode,
};
pub use process::{Output, run_until};
pub use registry::{
    BUILTINS, Check, Command, Domain, Effect, GLOBAL_FLAGS, SHARED_WORDS, SYNONYM_FLAGS, VERBS,
    check_layout, check_registry,
};
#[doc(hidden)]
pub use registry::{args_of, invoke, returns_of};
pub use search::{Quality, quality};
pub use secret::{Secret, redact, redact_value};
pub use when::{Span, When, now, utc, utc_time};
