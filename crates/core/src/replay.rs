//! Recorded HTTP for agent trials, compiled in only with the `fixtures`
//! feature (never in a release install).
//!
//! With `AGENT_CLI_FIXTURES=<dir>` every request is answered from the
//! recordings in `<dir>/http/*.json` rather than the network, and the cache
//! is off, so a trial sees exactly the world on disk. `AGENT_CLI_NOW` freezes
//! the clock (see [`crate::now`]), so relative times such as `--since 7d`
//! resolve to the same recorded URLs on any day.
//!
//! Each file is a JSON array of exchanges:
//!
//! ```json
//! [{"request": "GET https://dev.azure.com/contoso/Fabrikam/_apis/build/builds/8812?api-version=7.1",
//!   "answer": {"id": 8812}},
//!  {"request": "POST https://dev.azure.com/contoso/Fabrikam/_apis/wit/wiql?$top=20000&api-version=7.1",
//!   "body": {"query": "SELECT …"}, "answer": {"workItems": []}},
//!  {"request": "GET https://kv.example/x", "status": 404, "text": "gone", "note": "why it is here"}]
//! ```
//!
//! A request matches on its method and URL, query parameters in any order.
//! A `body` (JSON or form fields) must then equal the one sent, keys in any
//! order; an exchange without one answers any body. `status` defaults to
//! 200, `headers` to none; `answer` is JSON, `text` a body as it is. A
//! request with no recording is an error naming the closest one, and the
//! body that was sent, which is what to record next.
//!
//! `AGENT_CLI_FIXTURES_MATCH=loose` is for trials, where an agent picks its
//! own windows and limits (`--since 2d`, `--limit 10`): a miss is answered by
//! the closest recording of the same method and path, whatever its query and
//! body, and a path with no recording at all by a plain 404, as a service
//! would for a thing that does not exist. Nothing says so: an agent in a
//! trial should see a service, not the harness. Tests stay strict.
//!
//! A query parameter or body field that names *what* is asked for, rather
//! than a window or a limit, is listed in the exchange's `exact` (`["path"]`
//! for a file read, `["searchText"]` for a search): loose matching never
//! answers with a recording whose value differs, so an unrecorded file is a
//! 404 rather than its neighbour's text.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::ctx::Setup;
use crate::http::{Body, Request, Response, Transport};
use crate::secret::Secret;

/// Swaps the transport for recordings when `AGENT_CLI_FIXTURES` is set.
pub(crate) fn from_env(mut setup: Setup) -> Setup {
    let Some(dir) = std::env::var_os("AGENT_CLI_FIXTURES").filter(|dir| !dir.is_empty()) else {
        return setup;
    };
    let loose = std::env::var("AGENT_CLI_FIXTURES_MATCH").is_ok_and(|mode| mode == "loose");
    setup.transport = Box::new(Replay::load(Path::new(&dir), loose));
    setup.cache_dir = None;
    setup
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recorded {
    request: String,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    headers: Map<String, Value>,
    #[serde(default)]
    answer: Option<Value>,
    #[serde(default)]
    text: Option<String>,
    /// Query parameters or body fields loose matching must not substitute.
    #[serde(default)]
    exact: Vec<String>,
    /// For the person reading the file.
    #[serde(default)]
    #[allow(dead_code)]
    note: Option<String>,
}

struct Exchange {
    /// `GET https://host/path?a=1&b=2`, query sorted.
    key: String,
    body: Option<Value>,
    status: u16,
    headers: Vec<(String, String)>,
    answer: String,
    exact: Vec<String>,
}

pub(crate) struct Replay {
    exchanges: Vec<Exchange>,
    /// Why the recordings could not be read; every request then fails with it.
    problem: Option<String>,
    /// A miss gets the closest recording of its method and path, or a 404.
    loose: bool,
}

impl Replay {
    pub(crate) fn load(dir: &Path, loose: bool) -> Self {
        let (exchanges, problem) = match read(dir) {
            Ok(exchanges) => (exchanges, None),
            Err(error) => (Vec::new(), Some(format!("{error:#}"))),
        };
        Self {
            exchanges,
            problem,
            loose,
        }
    }

    /// The recording of `wanted`'s method and path whose query and body are
    /// closest to the ones sent; with `named`, only one whose `exact` fields
    /// name what was asked for.
    fn closest_on_path(
        &self,
        wanted: &str,
        body: Option<&Value>,
        named: bool,
    ) -> Option<&Exchange> {
        let path = |key: &str| key.split('?').next().unwrap_or_default().to_owned();
        let sent = format!(
            "{wanted} {}",
            body.map(Value::to_string).unwrap_or_default()
        );
        self.exchanges
            .iter()
            .filter(|exchange| path(&exchange.key) == path(wanted))
            .filter(|exchange| !named || !exchange.exact.is_empty())
            .filter(|exchange| {
                exchange.exact.iter().all(|name| {
                    field(&exchange.key, exchange.body.as_ref(), name) == field(wanted, body, name)
                })
            })
            .map(|exchange| {
                let recorded = format!(
                    "{} {}",
                    exchange.key,
                    exchange
                        .body
                        .as_ref()
                        .map(Value::to_string)
                        .unwrap_or_default()
                );
                (strsim::normalized_levenshtein(&recorded, &sent), exchange)
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, exchange)| exchange)
    }
}

/// A query parameter's value in `key`, else a top-level field of `body`.
fn field(key: &str, body: Option<&Value>, name: &str) -> Option<String> {
    let query = key.split_once('?').map_or("", |(_, query)| query);
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .map(str::to_owned)
        .or_else(|| body.and_then(|body| body.get(name)).map(Value::to_string))
}

fn read(dir: &Path) -> Result<Vec<Exchange>> {
    let http = dir.join("http");
    let mut files: Vec<_> = std::fs::read_dir(&http)
        .with_context(|| format!("AGENT_CLI_FIXTURES: cannot read {}", http.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    let mut exchanges = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("cannot read {}", file.display()))?;
        let recorded: Vec<Recorded> = serde_json::from_str(&text)
            .with_context(|| format!("{} is not a list of exchanges", file.display()))?;
        for entry in recorded {
            let (method, url) = entry.request.split_once(' ').with_context(|| {
                format!(
                    "{}: {:?} is not `METHOD URL`",
                    file.display(),
                    entry.request
                )
            })?;
            exchanges.push(Exchange {
                key: key(method, url),
                body: entry.body.map(normal),
                status: entry.status.unwrap_or(200),
                headers: entry
                    .headers
                    .into_iter()
                    .map(|(name, value)| {
                        let value = value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_owned);
                        (name, value)
                    })
                    .collect(),
                answer: match (entry.text, entry.answer) {
                    (Some(text), _) => text,
                    (None, Some(answer)) => answer.to_string(),
                    (None, None) => String::new(),
                },
                exact: entry.exact,
            });
        }
    }
    Ok(exchanges)
}

/// `METHOD url` with the query parameters sorted, so their order is not part
/// of the match.
fn key(method: &str, url: &str) -> String {
    let url = url.trim().split('#').next().unwrap_or_default();
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let mut pairs: Vec<&str> = query.split('&').filter(|pair| !pair.is_empty()).collect();
    pairs.sort_unstable();
    let query = if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    };
    format!("{} {path}{query}", method.trim().to_ascii_uppercase())
}

/// A JSON value with every object's keys sorted, so key order is not part of
/// the match either.
fn normal(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, normal(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normal).collect()),
        other => other,
    }
}

fn sent_body(body: &Body) -> Option<Value> {
    match body {
        Body::None => None,
        Body::Json(document) => Some(normal(document.clone())),
        Body::Form(fields) => Some(normal(Value::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                .collect(),
        ))),
    }
}

impl Transport for Replay {
    fn send(&self, request: &Request<'_>, _: Option<&Secret>, _: Duration) -> Result<Response> {
        if let Some(problem) = &self.problem {
            return Err(anyhow!("{problem}"));
        }
        let wanted = key(request.method.wire(), &request.url);
        let body = sent_body(&request.body);
        let same_url: Vec<&Exchange> = self
            .exchanges
            .iter()
            .filter(|exchange| exchange.key == wanted)
            .collect();
        let loose = |named| {
            self.loose
                .then(|| self.closest_on_path(&wanted, body.as_ref(), named))
                .flatten()
        };
        // Loosely, a recording that names what was asked for beats one that
        // answers any body.
        let found = same_url
            .iter()
            .find(|exchange| exchange.body.is_some() && exchange.body == body)
            .copied()
            .or_else(|| loose(true))
            .or_else(|| {
                same_url
                    .iter()
                    .find(|exchange| exchange.body.is_none())
                    .copied()
            })
            .or_else(|| loose(false));
        let Some(exchange) = found else {
            if self.loose {
                return Ok(Response {
                    status: 404,
                    headers: Vec::new(),
                    body: r#"{"message":"Not Found"}"#.to_owned(),
                    url: request.url.clone(),
                });
            }
            return Err(miss(
                &self.exchanges,
                &wanted,
                body.as_ref(),
                !same_url.is_empty(),
            ));
        };
        Ok(Response {
            status: exchange.status,
            headers: exchange.headers.clone(),
            body: exchange.answer.clone(),
            url: request.url.clone(),
        })
    }
}

/// No recording for `wanted`: the closest key, and the body to record.
fn miss(
    exchanges: &[Exchange],
    wanted: &str,
    body: Option<&Value>,
    same_url: bool,
) -> anyhow::Error {
    let closest = exchanges
        .iter()
        .map(|exchange| {
            (
                strsim::normalized_levenshtein(&exchange.key, wanted),
                &exchange.key,
            )
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map_or_else(|| "none (no recordings)".to_owned(), |(_, key)| key.clone());
    let sent = body.map_or_else(String::new, |body| format!("; body sent: {body}"));
    if same_url {
        anyhow!("fixtures: {wanted} is recorded, but not with this body{sent}")
    } else {
        anyhow!("fixtures: no recorded answer for {wanted}; closest recorded: {closest}{sent}")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::http::Method;

    fn replay(files: &[(&str, &str)]) -> Replay {
        load(files, false)
    }

    fn load(files: &[(&str, &str)], loose: bool) -> Replay {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("http")).unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join("http").join(name), text).unwrap();
        }
        Replay::load(dir.path(), loose)
    }

    fn send(replay: &Replay, request: Request<'_>) -> Result<Response> {
        replay.send(&request, None, Duration::from_secs(1))
    }

    #[test]
    fn a_request_is_matched_on_method_url_and_body_in_any_order() {
        let replay = replay(&[(
            "a.json",
            r#"[
              {"request": "GET https://h.example/x?b=2&a=1", "answer": {"ok": 1}},
              {"request": "POST https://h.example/q", "body": {"query": "one", "top": 5}, "answer": ["one"]},
              {"request": "POST https://h.example/q", "answer": ["any"]},
              {"request": "GET https://h.example/gone", "status": 404, "text": "gone",
               "headers": {"Retry-After": "1"}, "note": "for the tests"}
            ]"#,
        )]);
        let got = send(&replay, Request::get("https://h.example/x?a=1&b=2")).unwrap();
        assert_eq!((got.status, got.body.as_str()), (200, r#"{"ok":1}"#));
        let exact = Request::query("https://h.example/q", json!({"top": 5, "query": "one"}));
        assert_eq!(send(&replay, exact).unwrap().body, r#"["one"]"#);
        let other = Request::query("https://h.example/q", json!({"query": "two"}));
        assert_eq!(send(&replay, other).unwrap().body, r#"["any"]"#);
        let gone = send(&replay, Request::get("https://h.example/gone")).unwrap();
        assert_eq!((gone.status, gone.header("retry-after")), (404, Some("1")));
    }

    #[test]
    fn a_miss_names_the_closest_recording_and_the_body_sent() {
        let replay = replay(&[(
            "a.json",
            r#"[{"request": "GET https://h.example/builds/8812?api-version=7.1", "answer": {}},
                {"request": "POST https://h.example/q", "body": {"query": "one"}, "answer": []}]"#,
        )]);
        let error = send(
            &replay,
            Request::get("https://h.example/builds/8813?api-version=7.1"),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            "fixtures: no recorded answer for GET https://h.example/builds/8813?api-version=7.1; \
             closest recorded: GET https://h.example/builds/8812?api-version=7.1"
        );
        let request =
            Request::new(Method::Query, "https://h.example/q").json(json!({"query": "two"}));
        let error = send(&replay, request).unwrap_err().to_string();
        assert_eq!(
            error,
            r#"fixtures: POST https://h.example/q is recorded, but not with this body; body sent: {"query":"two"}"#
        );
    }

    #[test]
    fn loose_never_substitutes_what_an_exact_field_names() {
        let replay = load(
            &[(
                "a.json",
                r#"[{"request": "GET https://h.example/items?path=/src/a.cs&v=1", "exact": ["path"], "answer": "a"},
                    {"request": "POST https://h.example/search", "body": {"searchText": "Foo", "top": 5},
                     "exact": ["searchText"], "answer": ["foo"]},
                    {"request": "POST https://h.example/search", "answer": []}]"#,
            )],
            true,
        );
        let get = |url: &str| send(&replay, Request::get(url)).unwrap();
        assert_eq!(
            get("https://h.example/items?path=/src/a.cs&v=2").body,
            r#""a""#
        );
        assert_eq!(
            get("https://h.example/items?path=/src/ab.cs&v=1").status,
            404,
            "a neighbouring file is not this one"
        );
        let search = |text: &str| {
            let request = Request::query(
                "https://h.example/search",
                serde_json::json!({"searchText": text, "top": 50}),
            );
            send(&replay, request).unwrap().body
        };
        assert_eq!(search("Foo"), r#"["foo"]"#);
        assert_eq!(search("Bar"), "[]", "an unrecorded search finds nothing");
    }

    #[test]
    fn loose_answers_a_miss_with_the_closest_query_and_body_on_its_path_or_a_404() {
        let files = [(
            "a.json",
            r#"[{"request": "GET https://h.example/runs?limit=50&since=2026-09-28", "answer": ["day"]},
                {"request": "GET https://h.example/runs?limit=50", "answer": ["all"]},
                {"request": "GET https://h.example/runs/7", "answer": {"id": 7}},
                {"request": "POST https://h.example/q", "body": {"query": "state = 'Active'"}, "answer": ["active"]},
                {"request": "POST https://h.example/q", "body": {"query": "state = 'Closed' and type = 'Bug'"}, "answer": ["bugs"]}]"#,
        )];
        let loose = load(&files, true);
        let got = send(
            &loose,
            Request::get("https://h.example/runs?limit=10&since=2026-09-27"),
        );
        assert_eq!(got.unwrap().body, r#"["day"]"#);
        let got = send(&loose, Request::get("https://h.example/runs?limit=10")).unwrap();
        assert_eq!(got.body, r#"["all"]"#);
        let wiql = json!({"query": "state in ('New', 'Active')"});
        let got = send(&loose, Request::query("https://h.example/q", wiql.clone())).unwrap();
        assert_eq!(got.body, r#"["active"]"#);
        let gone = send(&loose, Request::get("https://h.example/runs/8")).unwrap();
        assert_eq!((gone.status, gone.body.contains("fixtures")), (404, false));

        let strict = load(&files, false);
        assert!(send(&strict, Request::get("https://h.example/runs?limit=10")).is_err());
        assert!(send(&strict, Request::query("https://h.example/q", wiql)).is_err());
    }

    #[test]
    fn broken_recordings_fail_every_request_with_the_reason() {
        let replay = replay(&[("a.json", r#"{"not": "a list"}"#)]);
        let error = send(&replay, Request::get("https://h.example/x")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("a.json is not a list of exchanges"),
            "{error}"
        );
    }
}
