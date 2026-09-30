//! Where a secret a config section needs comes from: the file itself
//! (discouraged), an environment variable, or a command such as
//! `pass show contoso/db` or `pup auth token`.
//!
//! A section keeps its own keys (sql's `password`, `password_env`,
//! `password_cmd`; a token's `token_env`, `token_cmd`) and hands the three to
//! [`Credential::from_keys`]. Nothing is read until [`Credential::resolve`],
//! so listing ten connections never runs ten commands.

use std::process::Command;

use anyhow::Result;

use crate::ctx::Ctx;
use crate::error::Failure;
use crate::secret::Secret;

#[derive(Clone, Debug)]
pub struct Credential {
    /// The config key it was named under: `password`, `token`, `api_key`.
    key: String,
    source: Source,
}

#[derive(Clone, Debug)]
enum Source {
    Value(Secret),
    Env(String),
    /// Run through `sh -c`; its stdout, less the trailing newline.
    Cmd(String),
}

impl Credential {
    /// The one source among `KEY`, `KEY_env` and `KEY_cmd`; `None` when none
    /// is set. Two is a mistake the message names.
    pub fn from_keys(
        key: &str,
        value: Option<String>,
        env: Option<String>,
        cmd: Option<String>,
    ) -> Result<Option<Self>, String> {
        let source = match (value, env, cmd) {
            (None, None, None) => return Ok(None),
            (Some(value), None, None) => Source::Value(Secret::new(value)),
            (None, Some(variable), None) => Source::Env(variable.trim().to_owned()),
            (None, None, Some(command)) => Source::Cmd(command),
            _ => return Err(format!("give one of {key}, {key}_env and {key}_cmd")),
        };
        Ok(Some(Self {
            key: key.to_owned(),
            source,
        }))
    }

    /// Where it comes from, never what it is: `password_env DB_PASS`,
    /// `token_cmd`, `password (in the config file)`. For doctor and listings.
    #[must_use]
    pub fn source(&self) -> String {
        match &self.source {
            Source::Value(_) => format!("{} (in the config file)", self.key),
            Source::Env(variable) => format!("{}_env {variable}", self.key),
            Source::Cmd(_) => format!("{}_cmd", self.key),
        }
    }

    /// The secret, read now: the variable through [`Ctx::env`], the command
    /// as a child process bounded by the deadline. Any problem is exit 3.
    pub fn resolve(&self, ctx: &Ctx) -> Result<Secret> {
        let key = &self.key;
        match &self.source {
            Source::Value(secret) => Ok(secret.clone()),
            Source::Env(variable) => ctx.env(variable).map(Secret::new).ok_or_else(|| {
                Failure::setup(format!("{key}_env {variable} is not set"))
                    .hint(format!(
                        "export {variable}, or name another source for {key}"
                    ))
                    .into()
            }),
            Source::Cmd(command) => {
                let mut sh = Command::new("sh");
                sh.args(["-c", command]);
                let output = ctx.read(sh)?;
                let said = output.stdout.trim_end_matches(['\r', '\n']);
                if !output.status.success() {
                    let why = output.stderr.trim();
                    return Err(Failure::setup(format!(
                        "{key}_cmd failed ({}): {why}",
                        output.status
                    ))
                    .hint(format!("run the {key}_cmd by hand to see why"))
                    .into());
                }
                if said.trim().is_empty() {
                    return Err(Failure::setup(format!("{key}_cmd printed nothing"))
                        .hint(format!(
                            "run the {key}_cmd by hand; it should print the {key}"
                        ))
                        .into());
                }
                Ok(Secret::new(said))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Setup;
    use crate::error::{Exit, describe};
    use crate::testing::{FakeTransport, ctx};

    fn credential(value: Option<&str>, env: Option<&str>, cmd: Option<&str>) -> Credential {
        let owned = |text: Option<&str>| text.map(str::to_owned);
        Credential::from_keys("token", owned(value), owned(env), owned(cmd))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn a_credential_comes_from_the_file_a_variable_or_a_command() {
        let ctx = ctx(Setup::fake(FakeTransport::default()).with_env("DD_TOKEN", "pat-from-env-1"));
        let read = |credential: Credential| credential.resolve(&ctx).map(|s| s.expose().to_owned());
        assert_eq!(
            read(credential(Some("in-file-1"), None, None)).unwrap(),
            "in-file-1"
        );
        assert_eq!(
            read(credential(None, Some("DD_TOKEN"), None)).unwrap(),
            "pat-from-env-1"
        );
        assert_eq!(
            read(credential(None, None, Some("printf 'from-cmd-1\\n'"))).unwrap(),
            "from-cmd-1",
            "the trailing newline is not part of it"
        );
        assert_eq!(
            credential(None, Some("DD_TOKEN"), None).source(),
            "token_env DD_TOKEN"
        );
        assert!(!format!("{:?}", credential(Some("in-file-1"), None, None)).contains("in-file-1"));

        for (credential, want) in [
            (
                credential(None, Some("UNSET_VAR"), None),
                "token_env UNSET_VAR is not set",
            ),
            (
                credential(None, None, Some("echo no >&2; exit 3")),
                "token_cmd failed",
            ),
            (
                credential(None, None, Some("true")),
                "token_cmd printed nothing",
            ),
        ] {
            let error = credential.resolve(&ctx).unwrap_err();
            let (exit, message, _) = describe(&error);
            assert_eq!(exit, Exit::Setup);
            assert!(message.starts_with(want), "{message}");
        }
        assert_eq!(
            Credential::from_keys("password", Some("a".into()), Some("B".into()), None)
                .unwrap_err(),
            "give one of password, password_env and password_cmd"
        );
        assert!(
            Credential::from_keys("password", None, None, None)
                .unwrap()
                .is_none()
        );
    }
}
