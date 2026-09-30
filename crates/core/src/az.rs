//! Tokens borrowed from the Azure CLI's login, for Azure DevOps and Azure alike.
//!
//! There is no credential of our own: `az login` has done the hard part, and
//! `az account get-access-token` mints a token for whichever resource is asked
//! for. Each one is kept in memory for the rest of the run.
// ponytail: no disk cache. One mint costs a Python start-up (about 250 ms
// measured for the signed-out path); add an expiry-aware 0600 cache if the
// signed-in cost measures above ~300 ms.

use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::ctx::{Ctx, locked};
use crate::error::Failure;
use crate::process::run_until;
use crate::secret::Secret;

/// The longest one mint may take. It is a second or two; one still going at
/// this is waiting on something that is not coming.
const AZ_CAP: Duration = Duration::from_secs(30);

impl Ctx {
    /// An access token for `resource` (for example `https://management.azure.com/`),
    /// minted by `az` on first use and again when `fresh` is set after a `401`.
    pub fn az_token(&self, resource: &str, fresh: bool) -> Result<Secret> {
        if !fresh && let Some(held) = locked(&self.tokens).get(resource) {
            return Ok(held.clone());
        }
        let deadline = self.deadline().min(Instant::now() + AZ_CAP);
        let minted = mint(Command::new("az"), resource, deadline)?;
        locked(&self.tokens).insert(resource.to_owned(), minted.clone());
        Ok(minted)
    }

    /// The `Request::auth` hook for an `az` token: `Bearer <token>`.
    pub fn az_bearer<'a>(&'a self, resource: &'a str) -> impl Fn(bool) -> Result<Secret> + 'a {
        move |fresh| {
            let token = self.az_token(resource, fresh)?;
            Ok(Secret::new(format!("Bearer {}", token.expose())))
        }
    }
}

/// One `az account get-access-token`, killed at `deadline`. `command` is `az`
/// itself, or a stand-in for it in a test.
fn mint(mut command: Command, resource: &str, deadline: Instant) -> Result<Secret> {
    command.args([
        "account",
        "get-access-token",
        "--resource",
        resource,
        "--query",
        "accessToken",
        "-o",
        "tsv",
    ]);
    let output = run_until(command, deadline)?;
    let token = output.stdout.trim();
    if output.status.success() && !token.is_empty() {
        return Ok(Secret::new(token));
    }
    let reason = output
        .stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it printed no token");
    Err(
        Failure::setup(format!("not signed in \u{2014} run `az login` ({reason})"))
            .hint("az login, then run the command again")
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Exit, describe};

    /// `sh -c script az`: the script sees the arguments `az` would, from `$1`.
    fn fake_az(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script, "az"]);
        command
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    #[test]
    fn a_signed_in_az_hands_back_its_token_for_the_resource_asked() {
        let az = fake_az(r#"[ "$4" = "https://vault.azure.net" ] && echo "  minted-az-token-1 ""#);
        let token = mint(az, "https://vault.azure.net", soon()).unwrap();
        assert_eq!(token.expose(), "minted-az-token-1");
    }

    #[test]
    fn a_signed_out_or_missing_az_is_needs_setup() {
        let az = fake_az("echo \"ERROR: Please run 'az login' to setup account.\" >&2; exit 1");
        let (exit, message, hint) = describe(&mint(az, "r", soon()).unwrap_err());
        assert_eq!(exit, Exit::Setup);
        assert_eq!(
            message,
            "not signed in \u{2014} run `az login` (ERROR: Please run 'az login' to setup account.)"
        );
        assert!(hint.unwrap().starts_with("az login"));

        let silent = mint(fake_az("exit 0"), "r", soon()).unwrap_err();
        assert!(describe(&silent).1.contains("printed no token"));

        let missing = mint(Command::new("agent-cli-no-such-az"), "r", soon()).unwrap_err();
        let (exit, message, _) = describe(&missing);
        assert_eq!(exit, Exit::Setup);
        assert!(message.contains("not installed or not on PATH"));
    }
}
