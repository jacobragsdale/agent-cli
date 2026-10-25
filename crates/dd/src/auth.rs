//! The Datadog credential: a bearer token or the `DD-API-KEY` and
//! `DD-APPLICATION-KEY` pair, held after the first request, read again from
//! Key Vault when a refused `*_kv` changed there, and attached only to
//! `api.<site>` at send time.

use std::sync::{MutexGuard, PoisonError};

use agent_cli_core::{
    Credential, Ctx, Exit, Failure, Method, Op, Request, Response, Secret, host_under, status_of,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::client::{Dd, NO_CREDENTIAL_HINT};

pub(crate) enum Auth {
    Token(Credential),
    Keys(Credential, Credential),
}

#[derive(Clone)]
pub(crate) enum Resolved {
    Bearer(Secret),
    Keys(Secret, Secret),
}

impl Dd {
    /// Where the credential comes from, never what it is: for doctor.
    pub(crate) fn source(&self) -> Option<String> {
        match &self.auth {
            None => None,
            Some(Auth::Token(token)) => Some(token.source()),
            Some(Auth::Keys(api, app)) => Some(format!("{} and {}", api.source(), app.source())),
        }
    }

    pub(crate) fn uses_keys(&self) -> bool {
        matches!(self.auth, Some(Auth::Keys(..)))
    }

    /// The credential, held after the first request; `fresh` (core's retry
    /// after a 401) takes a changed `*_kv` from [`Dd::renew`], else reads
    /// every source again.
    pub(crate) fn resolve(&self, ctx: &Ctx, fresh: bool) -> Result<Resolved> {
        if fresh && let Some(renewed) = self.renew(ctx)? {
            return Ok(renewed);
        }
        let mut held = self.held();
        if !fresh && let Some(resolved) = held.as_ref() {
            return Ok(resolved.clone());
        }
        let resolved = match &self.auth {
            None => {
                return Err(Failure::setup("no Datadog credential is set up")
                    .hint(NO_CREDENTIAL_HINT)
                    .into());
            }
            Some(Auth::Token(token)) => Resolved::Bearer(token.resolve(ctx)?),
            Some(Auth::Keys(api, app)) => Resolved::Keys(api.resolve(ctx)?, app.resolve(ctx)?),
        };
        *held = Some(resolved.clone());
        Ok(resolved)
    }

    /// After Datadog refused the credential: what a `*_kv` changed to in Key
    /// Vault, held now for every later request; `None` when nothing changed.
    pub(crate) fn renew(&self, ctx: &Ctx) -> Result<Option<Resolved>> {
        let renewed = match &self.auth {
            None => None,
            Some(Auth::Token(token)) => token.refresh(ctx)?.map(Resolved::Bearer),
            Some(Auth::Keys(api, app)) => match (api.refresh(ctx)?, app.refresh(ctx)?) {
                (None, None) => None,
                (api_key, app_key) => Some(Resolved::Keys(
                    api_key.map_or_else(|| api.resolve(ctx), Ok)?,
                    app_key.map_or_else(|| app.resolve(ctx), Ok)?,
                )),
            },
        };
        if renewed.is_some() {
            self.held().clone_from(&renewed);
        }
        Ok(renewed)
    }

    fn held(&self) -> MutexGuard<'_, Option<Resolved>> {
        self.resolved.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One request with the Datadog credential, attached when it is performed:
/// after core's `--dry-run` and `--yes` checks, and only to `api.<site>`.
pub(crate) struct Call<'a> {
    pub(crate) dd: &'a Dd,
    pub(crate) method: Method,
    pub(crate) url: String,
    pub(crate) body: Option<Value>,
}

impl Call<'_> {
    fn request<'r>(&self) -> Request<'r> {
        let request =
            Request::new(self.method, self.url.clone()).header("Accept", "application/json");
        match &self.body {
            Some(body) => request.json(body.clone()),
            None => request,
        }
    }
}

impl Op for Call<'_> {
    type Output = Response;

    /// The request, with each credential header it would carry shown as `***`.
    fn plan(&self) -> Value {
        let mut plan = self.request().plan();
        let headers: &[&str] = match &self.dd.auth {
            None => &[],
            Some(Auth::Token(_)) => &["Authorization"],
            Some(Auth::Keys(..)) => &["DD-API-KEY", "DD-APPLICATION-KEY"],
        };
        for name in headers {
            plan["headers"][*name] = json!("***");
        }
        plan
    }

    fn writes(&self) -> bool {
        !self.method.is_read()
    }

    /// Sent once more when Datadog refused a `*_kv` that has since changed in
    /// Key Vault: a 403 (bare for a key it does not know), or a 401 on keys
    /// (core already retries a token's 401 through its mint).
    fn perform(self, ctx: &Ctx) -> Result<Response> {
        match self.send(ctx) {
            Err(error)
                if (status_of(&error) == Some(403)
                    || (status_of(&error) == Some(401) && self.dd.uses_keys()))
                    && self.dd.renew(ctx)?.is_some() =>
            {
                self.send(ctx)
            }
            answer => answer,
        }
    }
}

impl Call<'_> {
    fn send(&self, ctx: &Ctx) -> Result<Response> {
        let api = format!("api.{}", self.dd.site.name);
        if !host_under(&self.url, &api) {
            bail!(
                "refusing to send the Datadog credential to {}; it goes only to {api}",
                self.url
            );
        }
        let request = self.request();
        match self.dd.resolve(ctx, false)? {
            Resolved::Bearer(_) => {
                let mint = |fresh: bool| match self.dd.resolve(ctx, fresh)? {
                    Resolved::Bearer(token) => {
                        Ok(Secret::new(format!("Bearer {}", token.expose())))
                    }
                    Resolved::Keys(..) => bail!("the Datadog credential changed kind"),
                };
                request.auth(&mint).perform(ctx)
            }
            Resolved::Keys(api_key, app_key) => request
                .header("DD-API-KEY", api_key.expose())
                .header("DD-APPLICATION-KEY", app_key.expose())
                .perform(ctx),
        }
    }
}

/// A `403` names a missing scope in Datadog's words, or is bare for a key
/// Datadog does not know: the hint names both, and doctor tells which.
pub(crate) fn forbidden(error: anyhow::Error) -> anyhow::Error {
    if status_of(&error) != Some(403) {
        return error;
    }
    let mut failure = Failure::new(Exit::Failed, format!("{error:#}")).hint(
        "the credential lacks a scope this call needs (a read-only token cannot mute), or Datadog does not know the key; `agent-cli doctor dd` checks which",
    );
    failure.status = Some(403);
    failure.into()
}
