//! The flags every request takes, as curl's do (the query, headers, sign-in,
//! TLS and proxy), and the one door requests go out by: the credential held
//! to the URL's origin, redirects followed by hand only when asked.

use std::path::PathBuf;
use std::time::Instant;

use agent_cli_core::{
    Body, Ctx, Effect, Failure, Method, Request, Response, Secret, Tls, base64, sensitive_key,
};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;

use crate::url;

/// The most redirects `--follow` takes.
const HOPS: usize = 10;

/// What a request command takes: the query, and how to reach the server.
#[derive(clap::Args, Clone, Debug, Default)]
pub(crate) struct Call {
    /// A query parameter K=V (repeatable)
    #[arg(long = "query", value_name = "K=V")]
    pub(crate) query: Vec<String>,
    #[command(flatten)]
    pub(crate) fetch: Fetch,
}

/// How to reach a server: headers, sign-in and TLS. The proxy is core's;
/// for one call, `HTTPS_PROXY` or `AGENT_CLI_CORE_PROXY=none` in its
/// environment.
#[derive(clap::Args, Clone, Debug, Default)]
pub(crate) struct Fetch {
    /// A header 'K: V' (repeatable)
    #[arg(long = "header", value_name = "K: V")]
    pub(crate) header: Vec<String>,
    /// A header from a variable, K=VAR: X-Api-Key=API_KEY
    #[arg(long = "header-env", value_name = "K=VAR")]
    pub(crate) header_env: Vec<String>,
    /// Bearer token from this variable
    #[arg(long = "bearer-env", value_name = "VAR")]
    pub(crate) bearer_env: Option<String>,
    /// Bearer token from the az login for this resource (api://APP-ID)
    #[arg(long = "az", value_name = "RESOURCE")]
    pub(crate) az: Option<String>,
    /// Basic sign-in as USER:VAR, the password from variable VAR
    #[arg(long = "basic", value_name = "USER:VAR")]
    pub(crate) basic: Option<String>,
    /// Trust these CAs (PEM) instead of the system store
    #[arg(long)]
    pub(crate) cacert: Option<PathBuf>,
    /// mTLS: a PEM with the client certificate and its key, or CERT,KEY
    #[arg(long, value_delimiter = ',', value_name = "PEM")]
    pub(crate) cert: Vec<PathBuf>,
    /// Accept any server certificate
    #[arg(long)]
    pub(crate) insecure: bool,
}

/// One redirect `--follow` took.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct Hop {
    pub(crate) status: u16,
    pub(crate) url: String,
    pub(crate) location: String,
    /// The hop left the first origin, so it went without the sign-in.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) unsigned: bool,
}

/// What came back from a call, redirects followed.
pub(crate) struct Exchange {
    pub(crate) response: Response,
    pub(crate) method: Method,
    pub(crate) url: String,
    pub(crate) ms: u64,
    pub(crate) hops: Vec<Hop>,
    /// The proxy it went through (`Via::url`).
    pub(crate) proxy: Option<String>,
}

/// A call's settings, read once for every request a command makes.
pub(crate) struct Prepared<'a> {
    headers: Vec<(String, String)>,
    secret_headers: Vec<(String, Secret)>,
    mint: Option<Box<dyn Fn(bool) -> Result<Secret> + 'a>>,
    tls: Option<Tls>,
    query: Vec<String>,
}

impl Call {
    pub(crate) fn prepare<'a>(&self, ctx: &'a Ctx) -> Result<Prepared<'a>> {
        let mut prepared = self.fetch.prepare(ctx)?;
        prepared.query.clone_from(&self.query);
        Ok(prepared)
    }
}

impl Fetch {
    /// Reads the sign-in, files and headers the flags name.
    pub(crate) fn prepare<'a>(&self, ctx: &'a Ctx) -> Result<Prepared<'a>> {
        let mut headers = Vec::new();
        let mut secret_headers = Vec::new();
        for raw in &self.header {
            let (name, value) = raw
                .split_once(':')
                .map(|(name, value)| (name.trim(), value.trim()))
                .filter(|(name, _)| !name.is_empty() && !name.contains(char::is_whitespace))
                .ok_or_else(|| {
                    Failure::usage(format!("--header {raw:?} is not 'NAME: VALUE'"))
                        .hint("--header 'Accept: application/json'")
                })?;
            if sensitive_key(name) {
                secret_headers.push((name.to_owned(), Secret::new(value.to_owned())));
            } else {
                headers.push((name.to_owned(), value.to_owned()));
            }
        }
        for raw in &self.header_env {
            let (name, variable) = raw
                .split_once('=')
                .map(|(name, variable)| (name.trim(), variable.trim()))
                .filter(|(name, variable)| !name.is_empty() && !variable.is_empty())
                .ok_or_else(|| {
                    Failure::usage(format!("--header-env {raw:?} is not NAME=VARIABLE"))
                        .hint("--header-env HEADER=VARIABLE, the header's value in the variable")
                })?;
            secret_headers.push((name.to_owned(), from_env(ctx, variable, "--header-env")?));
        }
        let signed = [
            self.bearer_env.is_some(),
            self.az.is_some(),
            self.basic.is_some(),
        ]
        .iter()
        .filter(|set| **set)
        .count();
        if signed > 1 {
            return Err(Failure::usage(
                "--bearer-env, --az and --basic each sign the call in; give one",
            )
            .into());
        }
        let has_authorization = secret_headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"));
        if signed == 1 && has_authorization {
            return Err(Failure::usage(
                "an Authorization header and a sign-in flag both sign the call in; give one",
            )
            .into());
        }
        let mint: Option<Box<dyn Fn(bool) -> Result<Secret> + 'a>> =
            if let Some(variable) = &self.bearer_env {
                let token = from_env(ctx, variable, "--bearer-env")?;
                let header = Secret::new(format!("Bearer {}", token.expose()));
                Some(Box::new(move |_| Ok(header.clone())))
            } else if let Some(resource) = &self.az {
                let resource = resource.trim().trim_end_matches("/.default").to_owned();
                Some(Box::new(move |fresh| {
                    let token = ctx.az_token(&resource, fresh)?;
                    Ok(Secret::new(format!("Bearer {}", token.expose())))
                }))
            } else if let Some(raw) = &self.basic {
                let (user, variable) = raw.split_once(':').ok_or_else(|| {
                    Failure::usage(format!("--basic {raw:?} is not USER:VARIABLE"))
                        .hint("--basic svc-orders:ORDERS_PASSWORD")
                })?;
                let password = from_env(ctx, variable, "--basic")?;
                let header = Secret::new(format!(
                    "Basic {}",
                    base64(format!("{user}:{}", password.expose()).as_bytes())
                ));
                Some(Box::new(move |_| Ok(header.clone())))
            } else {
                None
            };
        let read = |path: &PathBuf, flag: &str| -> Result<Vec<u8>> {
            std::fs::read(path)
                .map_err(|why| Failure::usage(format!("{flag} {}: {why}", path.display())).into())
        };
        let roots = self
            .cacert
            .as_ref()
            .map(|path| read(path, "--cacert"))
            .transpose()?;
        // One PEM may hold the chain and the key; ureq reads each from it.
        let client = match &self.cert[..] {
            [] => None,
            [both] => {
                let pem = read(both, "--cert")?;
                let key = String::from_utf8(pem.clone())
                    .map_err(|_| Failure::usage("--cert is not a PEM text file"))?;
                Some((pem, Secret::new(key)))
            }
            [cert, key] => {
                let key = String::from_utf8(read(key, "--cert")?)
                    .map_err(|_| Failure::usage("--cert's key is not a PEM text file"))?;
                Some((read(cert, "--cert")?, Secret::new(key)))
            }
            _ => {
                return Err(Failure::usage(
                    "--cert takes one PEM (certificate and key) or CERT,KEY",
                )
                .hint("--cert client.pem,client-key.pem")
                .into());
            }
        };
        let tls = (roots.is_some() || client.is_some() || self.insecure).then_some(Tls {
            roots,
            client,
            insecure: self.insecure,
        });
        if self.insecure {
            ctx.note("[--insecure: the server's certificate was not checked]");
        }
        Ok(Prepared {
            headers,
            secret_headers,
            mint,
            tls,
            query: Vec::new(),
        })
    }
}

fn from_env(ctx: &Ctx, variable: &str, flag: &str) -> Result<Secret> {
    ctx.env(variable).map(Secret::new).ok_or_else(|| {
        Failure::setup(format!(
            "{flag}: the variable {variable} is not set (or empty)"
        ))
        .hint(format!("export {variable}=…, then run it again"))
        .into()
    })
}

impl Prepared<'_> {
    /// `url` with `--query` added.
    pub(crate) fn url(&self, raw: &str) -> Result<String> {
        url::with_query(&url::checked(raw)?, &self.query)
    }

    /// A request carrying the call's settings, the sign-in only when
    /// `signed` (the first origin).
    fn request(&self, method: Method, url: &str, body: &Body, signed: bool) -> Request<'_> {
        let mut request = Request::new(method, url).any_status();
        request.body = body.clone();
        request.headers.clone_from(&self.headers);
        if signed {
            request.secret_headers.clone_from(&self.secret_headers);
            if let Some(mint) = &self.mint {
                request = request.auth(&**mint);
            }
        }
        if let Some(tls) = &self.tls {
            request = request.tls(tls.clone());
        }
        request
    }

    /// Sends one call, and follows its redirects when `follow`: a 303 (and a
    /// 301 or 302 after a POST) becomes a GET without the body, a 307 or 308
    /// repeats the method. A hop to another origin goes unsigned.
    pub(crate) fn send(
        &self,
        ctx: &Ctx,
        method: Method,
        url: &str,
        body: Body,
        follow: bool,
    ) -> Result<Exchange> {
        let first = url::origin(url);
        let proxy = ctx
            .via(url)?
            .map(|via| format!("{} ({})", via.url, via.from));
        let started = Instant::now();
        let (mut method, mut url, mut body) = (method, url.to_owned(), body);
        let mut hops = Vec::new();
        loop {
            let signed = url::origin(&url) == first;
            let request = self.request(method, &url, &body, signed);
            let response = if method.is_read() {
                ctx.read(request)?
            } else {
                let effect = if method == Method::Delete {
                    Effect::Destructive
                } else {
                    Effect::Write
                };
                ctx.write(effect, request)?
            };
            let location = response.header("location").map(str::to_owned);
            let redirect = matches!(response.status, 301 | 302 | 303 | 307 | 308);
            let (true, true, Some(location)) = (follow, redirect, location) else {
                return Ok(Exchange {
                    response,
                    method,
                    url,
                    ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    hops,
                    proxy,
                });
            };
            let next = url::join(&url, &location);
            if hops.len() >= HOPS || hops.iter().any(|hop: &Hop| hop.url == next) {
                let why = if hops.len() >= HOPS {
                    format!("more than {HOPS} redirects")
                } else {
                    "a redirect loop".to_owned()
                };
                return Err(Failure::new(
                    agent_cli_core::Exit::Failed,
                    format!("{} {url} led to {why}, at {next}", method.wire()),
                )
                .with_data(&hops)
                .into());
            }
            hops.push(Hop {
                status: response.status,
                url: url.clone(),
                location: next.clone(),
                unsigned: url::origin(&next) != first,
            });
            let keeps = matches!(response.status, 307 | 308)
                || (method != Method::Post && response.status != 303);
            if !keeps {
                method = Method::Get;
                body = Body::None;
            }
            url = next;
        }
    }
}

/// The body `--body`, `--body-file` and `--form` give, and the type it goes
/// as.
pub(crate) fn body(
    ctx: &Ctx,
    text: Option<&str>,
    file: Option<&std::path::Path>,
    content_type: Option<&str>,
    form: &[String],
) -> Result<(Body, Option<String>)> {
    let long = ctx.long_text("body", text, file, Some(16 * 1024 * 1024))?;
    if !form.is_empty() {
        if long.is_some() {
            return Err(Failure::usage("--form and --body each give the body; give one").into());
        }
        let fields = form
            .iter()
            .map(|pair| {
                pair.split_once('=')
                    .map(|(key, value)| (key.to_owned(), value.to_owned()))
                    .ok_or_else(|| Failure::usage(format!("--form {pair:?} is not K=V")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok((Body::Form(fields), None));
    }
    let Some(long) = long else {
        return Ok((Body::None, content_type.map(str::to_owned)));
    };
    let json_type = content_type.is_none_or(|kind| kind.to_ascii_lowercase().contains("json"));
    if json_type {
        let document = serde_json::from_str(&long.text)
            .with_context(|| "the body is not JSON")
            .map_err(|why| {
                Failure::usage(format!("{why:#}"))
                    .hint("send another kind with --content-type, e.g. --content-type text/plain")
            })?;
        return Ok((Body::Json(document), content_type.map(str::to_owned)));
    }
    Ok((
        Body::Bytes(long.text.into_bytes()),
        content_type.map(str::to_owned),
    ))
}

impl Prepared<'_> {
    /// Adds a `Content-Type` the body flags asked for.
    pub(crate) fn typed(mut self, content_type: Option<String>) -> Self {
        if let Some(kind) = content_type {
            self.headers
                .retain(|(name, _)| !name.eq_ignore_ascii_case("content-type"));
            self.headers.push(("Content-Type".to_owned(), kind));
        }
        self
    }
}
