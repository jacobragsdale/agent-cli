//! Where a secret a config section needs comes from: the file itself, so
//! one file holds a whole setup, an environment variable, a command such
//! as `pass show contoso/db` or `pup auth token`, or a team's Key Vault.
//!
//! A section keeps its own keys (sql's `password`, `password_env`,
//! `password_cmd`, `password_kv`; a token's `token`, `token_env`, …) and
//! hands the four to [`Credential::from_keys`]. Nothing is read until
//! [`Credential::resolve`], so listing ten connections never runs ten
//! commands.

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
    /// `VAULT/SECRET` as written, and its two names: read from Key Vault
    /// once, then kept in `secrets.toml` (see `vault.rs`).
    Kv {
        reference: String,
        vault: String,
        secret: String,
    },
}

impl Credential {
    /// The one source among `KEY`, `KEY_env`, `KEY_cmd` and `KEY_kv`; `None`
    /// when none is set. Two, a blank name or command (later "export , or
    /// …"), or a `KEY_kv` that is not `VAULT/SECRET` is a mistake the message
    /// names.
    pub fn from_keys(
        key: &str,
        value: Option<String>,
        env: Option<String>,
        cmd: Option<String>,
        kv: Option<String>,
    ) -> Result<Option<Self>, String> {
        for (suffix, given) in [("env", &env), ("cmd", &cmd), ("kv", &kv)] {
            if given.as_ref().is_some_and(|given| given.trim().is_empty()) {
                return Err(format!(
                    "{key}_{suffix} is empty; name the {} that gives the {key}, or leave {key}_{suffix} out",
                    match suffix {
                        "env" => "variable",
                        "cmd" => "command",
                        _ => "VAULT/SECRET",
                    }
                ));
            }
        }
        let source = match (value, env, cmd, kv) {
            (None, None, None, None) => return Ok(None),
            (Some(value), None, None, None) => Source::Value(Secret::new(value)),
            (None, Some(variable), None, None) => Source::Env(variable.trim().to_owned()),
            (None, None, Some(command), None) => Source::Cmd(command),
            (None, None, None, Some(reference)) => {
                let (vault, secret) = crate::vault::parse(key, &reference)?;
                Source::Kv {
                    reference: format!("{vault}/{secret}"),
                    vault,
                    secret,
                }
            }
            _ => {
                return Err(format!(
                    "give one of {key}, {key}_env, {key}_cmd and {key}_kv"
                ));
            }
        };
        Ok(Some(Self {
            key: key.to_owned(),
            source,
        }))
    }

    /// Where it comes from, never what it is: `password_env DB_PASS`,
    /// `token_cmd`, `password in the config file`. For doctor and listings.
    #[must_use]
    pub fn source(&self) -> String {
        match &self.source {
            Source::Value(_) => format!("{} in the config file", self.key),
            Source::Env(variable) => format!("{}_env {variable}", self.key),
            Source::Cmd(_) => format!("{}_cmd", self.key),
            Source::Kv { reference, .. } => format!("{}_kv {reference}", self.key),
        }
    }

    /// After the service refused the value (a 401, SQL Server 18456,
    /// ORA-01017, Redis WRONGPASS): a `KEY_kv` reads Key Vault again and,
    /// when the value changed there, keeps it and returns it for the one
    /// retry. `None` (any other source, or an unchanged value) means the
    /// refusal stands.
    pub fn refresh(&self, ctx: &Ctx) -> Result<Option<Secret>> {
        let Source::Kv {
            reference,
            vault,
            secret,
        } = &self.source
        else {
            return Ok(None);
        };
        let old = crate::vault::kept(ctx, reference);
        let new = crate::vault::read(ctx, &self.key, vault, secret)?;
        if old.is_some_and(|old| old.expose() == new.expose()) {
            return Ok(None);
        }
        crate::vault::keep(ctx, reference, &new)?;
        Ok(Some(new))
    }

    /// The secret, read now: the variable through [`Ctx::env`], the command
    /// as a child process bounded by the deadline, a `KEY_kv` as kept, else
    /// from Key Vault (then kept). Any problem is exit 3.
    pub fn resolve(&self, ctx: &Ctx) -> Result<Secret> {
        let key = &self.key;
        match &self.source {
            Source::Value(secret) => Ok(secret.clone()),
            Source::Kv {
                reference,
                vault,
                secret,
            } => {
                if let Some(kept) = crate::vault::kept(ctx, reference) {
                    return Ok(kept);
                }
                let value = crate::vault::read(ctx, key, vault, secret)?;
                crate::vault::keep(ctx, reference, &value)?;
                Ok(value)
            }
            Source::Env(variable) => ctx.env(variable).map(Secret::new).ok_or_else(|| {
                Failure::setup(format!("{key}_env {variable} is not set"))
                    .hint(format!(
                        "export {variable}, or name another source for {key}"
                    ))
                    .into()
            }),
            Source::Cmd(command) => {
                let output = ctx.read(shell(command))?;
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

/// `sh -c COMMAND`, or on Windows `cmd /D /S /C "COMMAND"`: `/S` takes the
/// line between the outer quotes as written, and `/D` keeps an AutoRun from
/// printing into the secret. Passed raw, as Rust's quoting is not cmd's.
fn shell(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/D", "/S", "/C"]);
        std::os::windows::process::CommandExt::raw_arg(&mut cmd, format!("\"{command}\""));
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut sh = Command::new("sh");
        sh.args(["-c", command]);
        sh
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
        Credential::from_keys("token", owned(value), owned(env), owned(cmd), None)
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
            Credential::from_keys("password", Some("a".into()), Some("B".into()), None, None)
                .unwrap_err(),
            "give one of password, password_env, password_cmd and password_kv"
        );
        assert!(
            Credential::from_keys("password", None, None, None, None)
                .unwrap()
                .is_none()
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_command_runs_under_cmd_with_its_quotes_as_written() {
        let ctx = ctx(Setup::fake(FakeTransport::default()));
        let secret = credential(None, None, Some(r#"echo "a b""#))
            .resolve(&ctx)
            .unwrap();
        assert_eq!(secret.expose(), r#""a b""#);
    }

    #[test]
    fn a_blank_variable_name_or_command_is_refused_naming_its_key() {
        let error = Credential::from_keys("token", None, Some(" ".into()), None, None).unwrap_err();
        assert!(
            error.starts_with("token_env is empty; name the variable"),
            "{error}"
        );
        let error =
            Credential::from_keys("token", None, None, Some(String::new()), None).unwrap_err();
        assert!(
            error.starts_with("token_cmd is empty; name the command"),
            "{error}"
        );
    }

    fn kv(reference: &str) -> Credential {
        Credential::from_keys("password", None, None, None, Some(reference.into()))
            .unwrap()
            .unwrap()
    }

    fn vault_answer(value: &str) -> crate::testing::Answer {
        crate::testing::Answer::json(&serde_json::json!({"value": value, "id": "x"}))
    }

    #[test]
    fn a_kv_credential_reads_key_vault_once_and_keeps_the_value_beside_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("secrets.toml");
        let transport = FakeTransport::answering([vault_answer("pw-from-vault-1")]);
        let setup = Setup::fake(transport.clone())
            .with_token("t")
            .with_secrets(&file);
        let ctx = ctx(setup);
        let credential = kv("kv-contoso-team/sql-reader");
        assert_eq!(
            credential.source(),
            "password_kv kv-contoso-team/sql-reader"
        );
        assert_eq!(
            credential.resolve(&ctx).unwrap().expose(),
            "pw-from-vault-1"
        );
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            "https://kv-contoso-team.vault.azure.net/secrets/sql-reader?api-version=7.4"
        );
        assert_eq!(
            sent[0].authorization.as_deref(),
            Some("Bearer t@https://vault.azure.net")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A later run reads the kept value and sends nothing.
        let quiet = FakeTransport::default();
        let later = crate::testing::ctx(Setup::fake(quiet.clone()).with_secrets(&file));
        assert_eq!(
            credential.resolve(&later).unwrap().expose(),
            "pw-from-vault-1"
        );
        assert!(quiet.sent().is_empty());
    }

    #[test]
    fn a_refresh_returns_a_changed_value_once_and_nothing_when_it_is_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("secrets.toml");
        let transport = FakeTransport::answering([
            vault_answer("pw-old-1"),
            vault_answer("pw-new-2"),
            vault_answer("pw-new-2"),
        ]);
        let ctx = ctx(Setup::fake(transport).with_secrets(&file));
        let shared = kv("kv-contoso-team/sql-reader");
        assert_eq!(shared.resolve(&ctx).unwrap().expose(), "pw-old-1");
        let fresh = shared.refresh(&ctx).unwrap().unwrap();
        assert_eq!(fresh.expose(), "pw-new-2");
        assert_eq!(shared.resolve(&ctx).unwrap().expose(), "pw-new-2");
        assert!(shared.refresh(&ctx).unwrap().is_none(), "unchanged");
        assert!(std::fs::read_to_string(&file).unwrap().contains("pw-new-2"));
        let literal = credential(Some("x"), None, None);
        assert!(
            literal.refresh(&ctx).unwrap().is_none(),
            "only a kv source refreshes"
        );
    }

    #[test]
    fn a_vault_that_refuses_is_needs_setup_naming_the_reference_and_the_role() {
        let transport = FakeTransport::answering([crate::testing::Answer::status(
            403,
            r#"{"error":{"message":"Forbidden by policy"}}"#,
        )]);
        let ctx = ctx(Setup::fake(transport));
        let error = kv("kv-contoso-team/sql-reader").resolve(&ctx).unwrap_err();
        let (exit, message, hint) = describe(&error);
        assert_eq!(exit, Exit::Setup);
        assert!(
            message.starts_with("password_kv kv-contoso-team/sql-reader could not be read"),
            "{message}"
        );
        assert!(hint.unwrap().contains("agent-cli config pull"));
        assert!(
            Credential::from_keys("password", None, None, None, Some("not a ref".into())).is_err()
        );
    }
}
