//! `http request`: one call to a URL, as curl makes it.

pub(crate) mod diff;
pub(crate) mod get;
pub(crate) mod run;
pub(crate) mod wait;

use agent_cli_core::{Failure, Method};
use anyhow::Result;

/// A method as typed, any case.
pub(crate) fn method(raw: &str) -> Result<Method> {
    Ok(match raw.to_ascii_uppercase().as_str() {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "OPTIONS" => Method::Options,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "PATCH" => Method::Patch,
        "DELETE" => Method::Delete,
        _ => {
            return Err(Failure::usage(format!(
                "{raw:?} is not GET, HEAD, OPTIONS, POST, PUT, PATCH or DELETE"
            ))
            .into());
        }
    })
}
