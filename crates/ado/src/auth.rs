//! The Azure DevOps credential: a PAT as `Basic` (read again from Key Vault
//! when a refused `pat_kv` changed there) or an az token as `Bearer`, where
//! it may go, and the ways Azure DevOps says it refused one.

use std::sync::{MutexGuard, PoisonError};

use agent_cli_core::{Credential, Ctx, Exit, Failure, Secret, base64, host_under};
use anyhow::Result;

use crate::client::Ado;

/// The resource `az account get-access-token` mints Azure DevOps tokens for.
const RESOURCE: &str = "499b84ac-1321-427f-aa17-267ca6975798";
pub(crate) const SIGN_IN_HINT: &str = "set pat_env or pat_cmd under [ado] (or AZURE_DEVOPS_EXT_PAT) to a personal access token, or run `az login`; `agent-cli doctor ado` checks the setup";

impl Ado {
    /// The `Authorization` value: the PAT as `Basic`, or an `az` token as
    /// `Bearer`, minted afresh when `fresh` says the last one was refused (a
    /// PAT then reads Key Vault again for a `pat_kv`, and stays as it was
    /// for any other source).
    pub(crate) fn authorization(&self, ctx: &Ctx, fresh: bool) -> Result<Secret> {
        if let Some(pat) = &self.pat {
            if fresh {
                self.renew(ctx)?;
            }
            if let Some(basic) = &*self.kept() {
                return Ok(basic.clone());
            }
            let made = basic(pat.resolve(ctx)?.expose().trim());
            *self.kept() = Some(made.clone());
            return Ok(made);
        }
        let token = ctx.az_token(RESOURCE, fresh)?;
        Ok(Secret::new(format!("Bearer {}", token.expose())))
    }

    /// After Azure DevOps refused the PAT: true when a `pat_kv` changed in Key
    /// Vault, kept now for every later request.
    pub(crate) fn renew(&self, ctx: &Ctx) -> Result<bool> {
        let Some(new) = self
            .pat
            .as_ref()
            .map(|pat| pat.refresh(ctx))
            .transpose()?
            .flatten()
        else {
            return Ok(false);
        };
        *self.kept() = Some(basic(new.expose().trim()));
        Ok(true)
    }

    fn kept(&self) -> MutexGuard<'_, Option<Secret>> {
        self.basic.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Where the PAT comes from, for doctor; `None` for an `az` token.
    pub(crate) fn pat_source(&self) -> Option<String> {
        self.pat.as_ref().map(Credential::source)
    }
}

/// Where an Azure DevOps credential may go: the organization's hosts and the
/// identity host, never a URL an answer handed back that points elsewhere.
pub(crate) fn trusted(url: &str) -> bool {
    host_under(url, "dev.azure.com") || host_under(url, ".visualstudio.com")
}

/// A sign-in page or a redirect to one: how Azure DevOps refuses a PAT
/// besides the 401 that core already retries.
pub(crate) fn refused_sign_in(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Failure>().is_some_and(|failure| {
        failure.exit == Exit::Setup
            && (failure.message.contains("answered 203")
                || failure.message.contains("which is not followed"))
    })
}

/// Core reads a refused credential as needs-setup; this names the two ways to
/// fix it for Azure DevOps. An organization backed by a Microsoft account
/// refuses with a redirect to its sign-in page instead of a 401, which is the
/// same answer.
pub(crate) fn signed_out(error: anyhow::Error) -> anyhow::Error {
    match error.downcast::<Failure>() {
        Ok(failure)
            if failure.exit == Exit::Setup || failure.message.contains("which is not followed") =>
        {
            Failure::setup(failure.message).hint(SIGN_IN_HINT).into()
        }
        // What is not there (deleted, a wrong id, another project's) names
        // the listing that shows what is.
        Ok(failure) if failure.hint.is_none() && failure.status == Some(404) => {
            let listing = if failure.message.contains("TF401232") {
                Some("agent-cli ado workitem list --text WORDS --fields id,title  (its id)")
            } else if failure.message.contains("TF401019") {
                Some("agent-cli ado repo list --fields name  (this project's repositories)")
            } else if failure.message.contains("requested build") {
                Some("agent-cli ado run list --fields id,pipeline,status")
            } else {
                None
            };
            match listing {
                Some(listing) => failure.hint(listing).into(),
                None => failure.into(),
            }
        }
        Ok(failure) => failure.into(),
        Err(error) => error,
    }
}

/// `Basic` credentials from a personal access token.
pub(crate) fn basic(pat: &str) -> Secret {
    // Made a Secret so redaction also masks the bare value.
    let _ = Secret::new(pat);
    Secret::new(format!("Basic {}", base64(format!(":{pat}").as_bytes())))
}
