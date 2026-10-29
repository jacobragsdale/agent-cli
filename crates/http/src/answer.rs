//! A response as a row: its status, headers and trace id, its body bounded
//! and masked, and a status that is not a success as the exit code it means.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Exit, Failure, Response, failure_message, redact_value};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::call::{Exchange, Hop};
use crate::url;

/// A string past this many characters is cut.
const STRING_MAX: usize = 2048;
/// A text body past this many bytes is cut.
const TEXT_MAX: usize = 16 * 1024;

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Answer {
    pub(crate) method: String,
    /// Where the answer came from: the last hop's URL under --follow.
    pub(crate) url: String,
    pub(crate) status: u16,
    pub(crate) reason: String,
    /// The whole call, redirects included.
    pub(crate) ms: u64,
    /// The proxy it went through, and where that was set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) proxy: Option<String>,
    /// Header names lowercase; a Set-Cookie's value is not shown.
    pub(crate) headers: BTreeMap<String, String>,
    /// The trace as `agent-cli dd span list --trace` takes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content_type: Option<String>,
    /// The body's size in bytes, as sent.
    pub(crate) size: usize,
    /// The redirects --follow took.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) hops: Vec<Hop>,
    /// Where it differs from the --spec's operation.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) spec_problems: Vec<String>,
    /// JSON parsed, arrays cut to --limit items and long strings cut, values
    /// that look like secrets masked; text cut at 16 KB; null for binary.
    pub(crate) body: Value,
    /// Where --output saved the whole body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) saved: Option<String>,
    /// The service's own words for a failure, from the body as sent.
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) said: String,
}

impl Answer {
    /// The row for an exchange, notes for what was cut or masked; `save`
    /// writes the whole body to `--output` when it is given.
    pub(crate) fn of(ctx: &Ctx, exchange: Exchange, limit: usize, save: bool) -> Result<Self> {
        let Exchange {
            response,
            method,
            url,
            ms,
            hops,
            proxy,
        } = exchange;
        let content_type = response.header("content-type").map(str::to_owned);
        let trace_id = trace_id(&response);
        let headers = response
            .headers
            .iter()
            .map(|(name, value)| {
                let name = name.to_ascii_lowercase();
                let value = if name == "set-cookie" {
                    let cookie = value.split('=').next().unwrap_or_default();
                    format!("{cookie}=…")
                } else {
                    value.clone()
                };
                (name, value)
            })
            .fold(
                BTreeMap::new(),
                |mut held: BTreeMap<String, String>, (name, value)| {
                    held.entry(name)
                        .and_modify(|seen| {
                            seen.push_str(", ");
                            seen.push_str(&value);
                        })
                        .or_insert(value);
                    held
                },
            );
        let size = response
            .bytes
            .as_ref()
            .map_or(response.body.len(), Vec::len);
        let saved = if save {
            ctx.save(&response.clone().into_bytes())?
                .map(|path| path.display().to_string())
        } else {
            None
        };
        let said = if response.status >= 400 {
            said(&response.body)
        } else {
            String::new()
        };
        let body = if saved.is_some() {
            Value::Null
        } else {
            shaped(ctx, &response, content_type.as_deref(), limit)
        };
        Ok(Self {
            method: method.wire().to_owned(),
            url,
            status: response.status,
            reason: reason(response.status).to_owned(),
            ms,
            proxy,
            headers,
            trace_id,
            content_type,
            size,
            hops,
            spec_problems: Vec::new(),
            body,
            saved,
            said,
        })
    }

    /// This row as the command's answer: as it is for a 1xx, 2xx or 3xx,
    /// else printed and then failed with the exit code its status means.
    pub(crate) fn finish(self, ctx: &Ctx) -> Result<Self> {
        if self.status < 400 {
            if (300..400).contains(&self.status)
                && let Some(location) = self.headers.get("location")
            {
                ctx.note(format!(
                    "[next: agent-cli http request get {}]",
                    url::join(&self.url, location)
                ));
            }
            return Ok(self);
        }
        let exit = match self.status {
            400 | 405 | 415 | 422 => Exit::Usage,
            401 | 407 => Exit::Setup,
            404 | 410 => Exit::NotFound,
            409 | 412 => Exit::Conflict,
            _ => Exit::Failed,
        };
        let said = self.said.trim();
        let said = if said.is_empty() || said.starts_with('<') {
            String::new()
        } else {
            format!(": {}", cut(said, 400))
        };
        let message = format!(
            "{} {} answered {} {}{said}",
            self.method, self.url, self.status, self.reason
        );
        let origin = url::origin(&self.url);
        let hint = match self.status {
            401 => Some(match self.headers.get("www-authenticate") {
                Some(asks) => format!(
                    "it asks for {asks}: sign in with --bearer-env VAR, --az RESOURCE, --basic USER:VAR, or a key header from --header-env"
                ),
                None => "sign in with --bearer-env VAR, --az RESOURCE, --basic USER:VAR, or a key header from --header-env".to_owned(),
            }),
            404 | 405 => Some(format!(
                "agent-cli http operation list {origin}  lists the operations its spec says it serves"
            )),
            400 | 422 => Some(format!(
                "the answer says what is wrong; agent-cli http operation list {origin}  finds the operation, and operation get what it takes"
            )),
            407 => Some("a proxy wants a sign-in: agent-cli doctor  names the proxy; set [core] proxy_user and proxy_password_env".to_owned()),
            415 => Some("the server wants another body type: set --content-type".to_owned()),
            429 => Some(match self.headers.get("retry-after") {
                Some(after) => format!("throttled: run it again after {after}s"),
                None => "throttled: run it again later".to_owned(),
            }),
            status if status >= 500 => self.trace_id.as_ref().map(|trace| {
                format!("the trace in Datadog: agent-cli dd span list --trace {trace}")
            }),
            _ => None,
        };
        let mut failure = Failure::new(exit, message);
        failure.status = Some(self.status);
        if let Some(hint) = hint {
            failure = failure.hint(hint);
        }
        Err(failure.with_data(self).into())
    }
}

/// The body as the row holds it.
fn shaped(ctx: &Ctx, response: &Response, content_type: Option<&str>, limit: usize) -> Value {
    if let Some(bytes) = &response.bytes {
        ctx.note(format!(
            "[a binary body of {} bytes ({}) is not shown; --output FILE saves it]",
            bytes.len(),
            content_type.unwrap_or("no Content-Type")
        ));
        return Value::Null;
    }
    let text = response.body.as_str();
    if text.is_empty() {
        return Value::Null;
    }
    let looks_json = content_type.is_some_and(|kind| kind.to_ascii_lowercase().contains("json"))
        || matches!(text.trim_start().chars().next(), Some('{' | '['));
    if looks_json && let Ok(document) = serde_json::from_str::<Value>(text) {
        let mut cuts = Vec::new();
        let bounded = bound(document, limit, &mut String::new(), &mut cuts);
        let masked = redact_value(bounded.clone());
        if masked != bounded {
            ctx.note(
                "[values that look like secrets are masked; --output FILE saves the body as sent]",
            );
        }
        if !cuts.is_empty() {
            let shown: Vec<&str> = cuts.iter().take(5).map(String::as_str).collect();
            let more = if cuts.len() > 5 { ", …" } else { "" };
            ctx.note(format!(
                "[cut: {}{more}; --limit N, or --output FILE for the whole body]",
                shown.join(", ")
            ));
        }
        return masked;
    }
    if text.len() > TEXT_MAX {
        let mut end = TEXT_MAX;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        ctx.note(format!(
            "[the text is cut at {} of {} bytes; --output FILE saves it]",
            end,
            text.len()
        ));
        return Value::String(text[..end].to_owned());
    }
    Value::String(text.to_owned())
}

/// `value` with every array cut to `limit` items and every long string cut,
/// each cut named by its path in `cuts` (`items[50 of 600]`).
fn bound(value: Value, limit: usize, path: &mut String, cuts: &mut Vec<String>) -> Value {
    match value {
        Value::Array(items) => {
            let total = items.len();
            let at = path.len();
            let kept: Vec<Value> = items
                .into_iter()
                .take(limit)
                .map(|item| {
                    path.push_str("[]");
                    let item = bound(item, limit, path, cuts);
                    path.truncate(at);
                    item
                })
                .collect();
            if total > limit {
                let name = if path.is_empty() {
                    "the body"
                } else {
                    path.as_str()
                };
                cuts.push(format!("{name}[{limit} of {total}]"));
            }
            Value::Array(kept)
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, item)| {
                    let at = path.len();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(&key);
                    let item = bound(item, limit, path, cuts);
                    path.truncate(at);
                    (key, item)
                })
                .collect(),
        ),
        Value::String(text) if text.chars().count() > STRING_MAX => {
            cuts.push(format!(
                "{} ({} characters)",
                if path.is_empty() { "the body" } else { path },
                text.chars().count()
            ));
            Value::String(format!(
                "{}…",
                text.chars().take(STRING_MAX).collect::<String>()
            ))
        }
        other => other,
    }
}

/// The service's own words for a failure: the shapes REST APIs write them
/// in (FastAPI's `detail`, problem+json's `title` and `detail`, `error`,
/// `message`), else core's reading.
fn said(body: &str) -> String {
    let Ok(document) = serde_json::from_str::<Value>(body) else {
        return failure_message(body);
    };
    let text = |value: &Value| {
        value
            .as_str()
            .map(str::to_owned)
            .filter(|s| !s.trim().is_empty())
    };
    // problem+json's title is mostly the reason phrase the message already
    // holds; its detail says what happened.
    let problem = text(&document["detail"]).or_else(|| text(&document["title"]));
    problem
        .or_else(|| text(&document["error"]["message"]))
        .or_else(|| text(&document["error"]))
        .or_else(|| text(&document["message"]))
        .or_else(|| text(&document["error_description"]))
        .unwrap_or_else(|| failure_message(body))
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    format!("{}…", text.chars().take(max).collect::<String>())
}

/// The trace a response carries, as Datadog's span search takes it:
/// `x-datadog-trace-id` as sent, else W3C `traceparent`'s trace id, whose
/// low 64 bits in decimal are what Datadog's tracers keep.
pub(crate) fn trace_id(response: &Response) -> Option<String> {
    if let Some(id) = response
        .header("x-datadog-trace-id")
        .map(str::trim)
        .filter(|id| id.parse::<u64>().is_ok())
    {
        return Some(id.to_owned());
    }
    let parent = response.header("traceparent")?.trim();
    let mut parts = parent.split('-');
    let (_, trace) = (parts.next()?, parts.next()?);
    if trace.len() != 32 || !trace.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(&trace[16..], 16)
        .ok()
        .filter(|low| *low != 0)
        .map(|low| low.to_string())
}

/// The reason phrase a status goes by.
pub(crate) fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(headers: &[(&str, &str)]) -> Response {
        Response {
            status: 503,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            ..Response::default()
        }
    }

    #[test]
    fn a_traceparent_becomes_the_decimal_trace_id_datadog_keeps() {
        let traced = response(&[(
            "traceparent",
            "00-0000000000000000199e6ab3c4d5e6f7-00f067aa0ba902b7-01",
        )]);
        assert_eq!(
            trace_id(&traced).as_deref(),
            Some(
                &*u64::from_str_radix("199e6ab3c4d5e6f7", 16)
                    .unwrap()
                    .to_string()
            )
        );
        let datadog = response(&[
            ("x-datadog-trace-id", "1846223019175527734"),
            ("traceparent", "00-ab-cd-01"),
        ]);
        assert_eq!(trace_id(&datadog).as_deref(), Some("1846223019175527734"));
        assert_eq!(trace_id(&response(&[("traceparent", "garbage")])), None);
    }

    #[test]
    fn arrays_are_cut_to_the_limit_and_long_strings_to_two_thousand_characters() {
        let mut cuts = Vec::new();
        let long = "x".repeat(STRING_MAX + 10);
        let value = serde_json::json!({"items": (0..5).collect::<Vec<_>>(), "note": long, "lines": [{"tags": [1, 2, 3]}]});
        let bounded = bound(value, 2, &mut String::new(), &mut cuts);
        assert_eq!(bounded["items"], serde_json::json!([0, 1]));
        assert_eq!(bounded["lines"][0]["tags"], serde_json::json!([1, 2]));
        assert_eq!(
            bounded["note"].as_str().unwrap().chars().count(),
            STRING_MAX + 1
        );
        assert_eq!(
            cuts,
            [
                "items[2 of 5]",
                &format!("note ({} characters)", STRING_MAX + 10),
                "lines[].tags[2 of 3]"
            ]
        );
    }
}
