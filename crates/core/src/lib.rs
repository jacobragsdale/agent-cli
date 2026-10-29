//! The runtime behind `agent-cli`: registry, dispatch, discovery, search,
//! output, safety, config, cache, child processes and HTTP.
//!
//! A domain crate exports `pub const DOMAIN: Domain` listing its commands,
//! each made with [`command!`]. Everything an agent relies on across domains
//! — the three-word paths, search, help, `--fields`, the output guard,
//! `--dry-run`, `--yes`, exit codes, redaction — lives here, so a new
//! command gets all of it by being registered.

mod az;
mod base_url;
mod cache;
mod config;
mod credential;
mod ctx;
mod diff;
mod discover;
mod dispatch;
mod doctor;
mod error;
mod handshake;
mod hidden;
mod http;
mod https;
mod layers;
mod leaf;
mod output;
mod process;
mod proxy;
mod registry;
#[cfg(feature = "fixtures")]
mod replay;
mod said;
mod search;
mod secret;
mod stop;
pub mod testing;
mod throttle;
mod tls;
mod usage;
mod vault;
mod when;

pub use anyhow;
pub use clap;
pub use schemars;
pub use serde_json;

pub use base_url::{check_base_url, same_origin};
pub use cache::Cache;
pub use config::{Config, pick};
pub use credential::Credential;
pub use ctx::{Ctx, DEFAULT_TIMEOUT, Globals, LongText, Op, Setup};
pub use diff::{Change, Diff, DiffArgs, Rules, Side, diff, scoped};
pub use discover::command_help;
pub use dispatch::{run, run_with};
pub use doctor::Check;
pub use error::{Exit, Failure, status_of};
pub use handshake::{Handshake, Peer};
pub use http::{
    Body, Method, Mint, Request, Response, Transport, base64, form_encode, host_under,
    percent_encode,
};
pub use https::Https;
pub use process::{Output, run_until};
pub use proxy::Via;
pub use registry::{
    BUILTINS, Command, Domain, Effect, GLOBAL_FLAGS, SHARED_WORDS, SYNONYM_FLAGS, VERBS,
    check_layout, check_registry,
};
#[doc(hidden)]
pub use registry::{args_of, invoke, returns_of};
pub use said::failure_message;
pub use search::{Quality, quality};
pub use secret::{Secret, redact, redact_value, sensitive_key};
pub use stop::{OnStop, on_stop};
pub use tls::{Tls, untrusted_hint};
pub use when::{Span, When, now, utc, utc_time};
