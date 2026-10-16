//! `kv secret update`: a new version of an existing secret (`PUT
//! /secrets/{name}`), keeping its tags and content type. The value comes
//! from stdin or a file, never the command line, where it would land in a
//! transcript; it lives in a `Secret` and goes out as JSON bytes, so the
//! `--dry-run` plan holds its length only. The answer is the new version's
//! id: the vault sends the value back, and it is dropped.

use std::collections::BTreeMap;
use std::path::PathBuf;

use agent_cli_core::{Ctx, Failure, Secret, Span, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{from_unix, text};
use crate::kv::{API_VERSION, base, holder, item_ref, last_segment, put, secrets, segment};

/// The service's own cap on a secret's value.
const LIMIT: usize = 25 * 1024;

#[derive(clap::Args)]
pub struct SecretUpdateArgs {
    /// The secret: its name, its id (vault/name) or its URI
    secret: String,
    /// - reads the new value from stdin (at most 25 KiB); never the value itself
    value: Option<String>,
    /// Read the new value from this file instead
    #[arg(long)]
    value_file: Option<PathBuf>,
    /// The vault that holds it; needed when more than one does
    #[arg(long)]
    vault: Option<String>,
    /// The new version's content type (text/plain …); the current one's when left out
    #[arg(long)]
    content_type: Option<String>,
    /// When the new version expires: a length from now (90d) or a date or RFC 3339 time
    #[arg(long)]
    expires: Option<String>,
}

/// The new version, named and described; never its value.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Updated {
    /// `vault/name/version`: what `kv secret get` takes for this version.
    id: String,
    vault: String,
    name: String,
    version: Option<String>,
    content_type: Option<String>,
    /// RFC 3339.
    expires: Option<String>,
    tags: BTreeMap<String, String>,
}

fn secret_update(ctx: &Ctx, args: SecretUpdateArgs) -> Result<Updated> {
    if args.value.as_deref().is_some_and(|value| value != "-") {
        return Err(Failure::usage(
            "a secret's value never goes on the command line, where a transcript keeps it",
        )
        .hint("pipe it in with - (agent-cli kv secret update NAME -), or pass --value-file PATH")
        .into());
    }
    let expires = args.expires.as_deref().map(expiry).transpose()?;
    let wanted = item_ref(&args.secret, "secrets", args.vault.as_deref(), None)?;
    if wanted.version.is_some() {
        return Err(Failure::usage(format!(
            "{} names one version; an update adds a version to the secret (vault/name)",
            args.secret
        ))
        .into());
    }
    let Some(text_in) = ctx.long_text(
        "value",
        args.value.as_deref(),
        args.value_file.as_deref(),
        Some(LIMIT),
    )?
    else {
        return Err(Failure::usage("the new value comes from stdin or a file")
            .hint(
                "pipe it in with - (agent-cli kv secret update NAME -), or pass --value-file PATH",
            )
            .into());
    };
    let value = Secret::new(text_in.text);
    let vault = holder(ctx, "secrets", &wanted.name, wanted.vault.as_deref())?;
    // An update keeps what describes the secret; a PUT alone would drop it.
    let Some(current) = secrets(ctx, &vault, false)?
        .into_iter()
        .find(|row| row.name.eq_ignore_ascii_case(&wanted.name))
    else {
        return Err(Failure::not_found(format!(
            "no secret {} in {}; update adds a version to one that exists",
            wanted.name, vault.name
        ))
        .hint(format!("agent-cli kv secret list --vault {}", vault.name))
        .into());
    };
    let content_type = args.content_type.or(current.content_type);
    let mut body = json!({"value": value.expose(), "tags": current.tags});
    if let Some(content_type) = &content_type {
        body["contentType"] = json!(content_type);
    }
    if let Some(expires) = expires {
        body["attributes"] = json!({"exp": expires.unix_timestamp()});
    }
    let url = format!(
        "{}secrets/{}?api-version={API_VERSION}",
        base(&vault),
        segment(&current.name)
    );
    let answer = put(ctx, &vault, &url, body.to_string().into_bytes())?;
    let version = last_segment(&answer);
    Ok(Updated {
        id: format!(
            "{}/{}/{}",
            vault.name,
            current.name,
            version.as_deref().unwrap_or_default()
        ),
        vault: vault.name.clone(),
        name: current.name,
        version,
        content_type: text(&answer["contentType"]),
        expires: from_unix(&answer["attributes"]["exp"]),
        tags: current.tags,
    })
}

/// `--expires`: a length from now, else a time; one already past is a usage
/// error, since it would make a version no one can use.
fn expiry(raw: &str) -> Result<time::OffsetDateTime> {
    let now = agent_cli_core::now();
    let at = match raw.parse::<Span>() {
        Ok(span) => now + span.0,
        Err(_) => raw
            .parse::<When>()
            .map_err(|_| {
                Failure::usage(format!(
                    "--expires takes a length from now (90d) or a date or RFC 3339 time, not {raw:?}"
                ))
            })?
            .0,
    };
    if at <= now {
        return Err(Failure::usage(format!(
            "--expires {raw} is already past; the new version would be expired"
        ))
        .into());
    }
    Ok(at)
}

command! {
    pub SECRET_UPDATE = ["kv", "secret", "update"], Write,
    "Set a Key Vault secret's new value (a new version) from stdin or a file",
    keywords: ["rotate", "rotation", "password", "credential", "set", "change", "new", "value", "version", "renew"],
    example: "kv secret update kv-contoso-prod/db-password --value-file new-password.txt --expires 90d",
    run: secret_update,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, assert_dry_run, run};
    use serde_json::json;

    use crate::KV;
    use crate::testing::{self, SERIAL, listing};

    const VALUE: &str = "n3w-fixture-value-7Qx";

    fn update(
        argv: &[&str],
        answers: Vec<Answer>,
    ) -> (agent_cli_core::testing::Outcome, FakeTransport) {
        let transport = FakeTransport::answering(answers);
        let outcome = run(
            &[KV],
            argv,
            Setup::fake(transport.clone())
                .with_config(SERIAL)
                .with_stdin(&format!("{VALUE}\n")),
        );
        (outcome, transport)
    }

    fn reads() -> Vec<Answer> {
        vec![
            testing::inventory(vec![testing::vault("kv-contoso-dev")]),
            listing("kv-contoso-dev", &["db-password"]),
        ]
    }

    #[test]
    fn an_update_puts_a_new_version_keeping_the_tags_and_answers_its_id_only() {
        let mut answers = reads();
        answers.push(Answer::json(&json!({
            "value": VALUE, "contentType": "text/plain",
            "id": "https://kv-contoso-dev.vault.azure.net/secrets/db-password/9d8c",
            "attributes": {"enabled": true, "exp": 1_803_895_200},
        })));
        let (outcome, transport) = update(
            &[
                "kv",
                "secret",
                "update",
                "kv-contoso-dev/db-password",
                "-",
                "--expires",
                "2027-03-01",
            ],
            answers,
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["id"], "kv-contoso-dev/db-password/9d8c");
        assert!(!outcome.stdout.contains(VALUE), "{}", outcome.stdout);
        assert!(!outcome.stderr.contains(VALUE), "{}", outcome.stderr);
        let sent = &transport.sent()[2];
        assert_eq!(
            sent.url,
            "https://kv-contoso-dev.vault.azure.net/secrets/db-password?api-version=7.4"
        );
        assert_eq!(
            sent.authorization.as_deref(),
            Some("Bearer token@https://vault.azure.net")
        );
        let body: serde_json::Value =
            serde_json::from_str(sent.body.as_ref().unwrap().as_str().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"value": VALUE, "contentType": "text/plain", "tags": {"owner": "platform"},
                "attributes": {"exp": 1_803_859_200}})
        );
    }

    #[test]
    fn the_dry_run_plan_and_a_refusal_never_hold_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("value.txt");
        std::fs::write(&file, VALUE).unwrap();
        let plans = assert_dry_run(
            &[KV],
            &[
                "kv",
                "secret",
                "update",
                "kv-contoso-dev/db-password",
                "--value-file",
                file.to_str().unwrap(),
            ],
            reads(),
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert!(!json!(plans).to_string().contains(VALUE), "{plans:?}");
        let (outcome, _) = update(
            &[
                "kv",
                "secret",
                "update",
                "kv-contoso-dev/db-password",
                "--dry-run",
                "-",
            ],
            reads(),
        );
        assert!(
            !outcome.stdout.contains(VALUE) && !outcome.stderr.contains(VALUE),
            "{outcome:?}"
        );

        let mut answers = reads();
        answers.push(Answer::status(
            403,
            r#"{"error":{"code":"Forbidden","message":"Caller is not authorized to perform action on resource."}}"#,
        ));
        let (outcome, _) = update(&["kv", "secret", "update", "db-password", "-"], answers);
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Key Vault Secrets Officer"),
            "{}",
            outcome.stderr
        );
        assert!(!outcome.stderr.contains(VALUE), "{}", outcome.stderr);
    }

    #[test]
    fn a_value_on_the_command_line_or_a_missing_secret_is_refused() {
        let (outcome, transport) =
            update(&["kv", "secret", "update", "db-password", VALUE], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
        assert!(!outcome.stderr.contains(VALUE), "{}", outcome.stderr);

        let (outcome, _) = update(
            &[
                "kv",
                "secret",
                "update",
                "db-password",
                "-",
                "--expires",
                "2020-01-01",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");

        let (outcome, _) = update(&["kv", "secret", "update", "nope", "-"], reads());
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
