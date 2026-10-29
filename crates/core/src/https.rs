//! The real transport: ureq over the OS trust store, one agent per proxy
//! and TLS setting, and the TLS handshake `http certificate` makes.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;

use crate::ctx::locked;
use crate::error::{Exit, Failure};
use crate::handshake::{Handshake, Peer};
use crate::http::{
    BODY_LIMIT, Body, CONNECT, MAY_HAVE_LANDED, Method, Request, Response, Transport, form_encode,
};
use crate::proxy::{proxy_refused, proxy_schemes};
use crate::secret::Secret;
use crate::tls::Tls;

/// The real transport. Redirects are not followed: a hop would drop the
/// `Authorization` header, or worse carry it, and trade a status this code can
/// read for a sign-in page it cannot.
#[derive(Default)]
pub struct Https {
    /// One per proxy and TLS setting, built on first use, so a command that
    /// never calls out never pays for the TLS setup.
    agents: Mutex<HashMap<String, ureq::Agent>>,
}

impl Https {
    fn agent(&self, request: &Request<'_>) -> Result<ureq::Agent> {
        let key = format!(
            "{}|{}|{}",
            request.via.as_ref().map_or("", |via| via.url.as_str()),
            request
                .via
                .as_ref()
                .and_then(|via| via.user.as_deref())
                .unwrap_or(""),
            request.tls.as_ref().map_or(0, Tls::key)
        );
        let mut agents = locked(&self.agents);
        if let Some(agent) = agents.get(&key) {
            return Ok(agent.clone());
        }
        let agent = crate::tls::agent(request.via.as_ref(), request.tls.as_ref())?;
        agents.insert(key, agent.clone());
        Ok(agent)
    }
}

impl Transport for Https {
    fn handshake(&self, target: &Handshake, timeout: Duration) -> Result<Peer> {
        crate::handshake::shake(target, timeout)
    }

    fn send(
        &self,
        request: &Request<'_>,
        authorization: Option<&Secret>,
        timeout: Duration,
    ) -> Result<Response> {
        let agent = self.agent(request)?;
        let url = request.url.as_str();
        let mut headers: Vec<(&str, &str)> = request
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        if let Some(authorization) = authorization {
            headers.push(("Authorization", authorization.expose()));
        }
        for (name, value) in &request.secret_headers {
            headers.push((name.as_str(), value.expose()));
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
            Method::Head => prepare(agent.head(url), &headers, timeout).call(),
            Method::Options => prepare(agent.options(url), &headers, timeout).call(),
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
            ureq::Error::ConnectProxyFailed(reason) => {
                let via = request.via.as_ref();
                let name = via.map_or_else(String::new, |via| format!(" {} ({})", via.url, via.from));
                let failure = Failure::new(
                    Exit::Failed,
                    format!("{} {url} failed: the proxy{name} {reason}", request.method.wire()),
                );
                // ponytail: ureq reaches every target through a proxy by
                // CONNECT, plain http:// too, which proxies often allow to
                // port 443 only; an absolute-form request would get past
                // that, if an http:// API outside no_proxy ever needs it.
                match via {
                    Some(via) if reason.contains(" 407") => anyhow::Error::new(Failure {
                        exit: Exit::Setup,
                        ..failure.hint(proxy_refused(via, &proxy_schemes(via, url, timeout)))
                    }),
                    Some(_) if reason.contains(" 403") => anyhow::Error::new(failure.hint(
                        "the proxy refused to tunnel there (proxies often tunnel to port 443 only): a host inside the network belongs in [core] no_proxy, or NO_PROXY",
                    )),
                    _ => anyhow::Error::new(failure),
                }
            }
            other => {
                let said = other.to_string();
                let through = request
                    .via
                    .as_ref()
                    .map_or_else(String::new, |via| format!(" (through the proxy {})", via.url));
                match crate::tls::untrusted(&said) {
                    Some(hint) => anyhow::Error::new(
                        Failure::setup(format!(
                            "{} {url} failed: this machine does not trust the server's certificate ({said})",
                            request.method.wire()
                        ))
                        .hint(hint),
                    ),
                    None => anyhow::Error::new(other)
                        .context(format!("{} {url} failed{through}", request.method.wire())),
                }
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
