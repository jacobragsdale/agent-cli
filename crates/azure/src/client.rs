//! What every Azure domain shares: `az` tokens and the hosts they may go to,
//! child processes, allowlists, bounded fan-out, and reading service stamps.

use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_cli_core::{Ctx, Failure, Secret};
use anyhow::Result;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The ARM audience. The trailing slash is part of it: ARM refuses a token
/// minted for the same URL without one.
pub(crate) const ARM: &str = "https://management.azure.com/";
pub(crate) const VAULT: &str = "https://vault.azure.net";
/// What `az acr` itself asks for. Not ARM's: a hardened registry turns off
/// `azureADAuthenticationAsArmPolicy` and refuses an ARM-scoped token.
pub(crate) const REGISTRY: &str = "https://containerregistry.azure.net";
/// AI Search's data plane, without a trailing slash, as `az` asks for it.
pub(crate) const SEARCH: &str = "https://search.azure.com";

/// `az`'s token for `resource` (`Setup::with_token` stands in for it in tests).
pub(crate) fn az_token(ctx: &Ctx, resource: &str, fresh: bool) -> Result<Secret> {
    ctx.az_token(resource, fresh)
}

/// The `Request::auth` hook for an `az` token.
pub(crate) fn bearer<'a>(ctx: &'a Ctx, resource: &'a str) -> impl Fn(bool) -> Result<Secret> + 'a {
    move |fresh| {
        let token = az_token(ctx, resource, fresh)?;
        Ok(Secret::new(format!("Bearer {}", token.expose())))
    }
}

/// A child process (`az`, `kubelogin`, `kubectl`). Tests put the repo's
/// `scripts/fake` first on its PATH: none of the three is usable where they
/// run.
pub(crate) fn program(name: &str) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(name);
    #[cfg(test)]
    command.env(
        "PATH",
        format!(
            "{}/../../scripts/fake:{}",
            env!("CARGO_MANIFEST_DIR"),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    command
}

/// The resources an allowlist names, in its order, matched without regard to
/// case. An empty (or all-blank) allowlist keeps everything.
pub(crate) fn allowed<T: Clone>(
    found: &[T],
    names: &[String],
    name_of: impl Fn(&T) -> &str,
) -> Vec<T> {
    let names: Vec<&str> = names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        return found.to_vec();
    }
    let mut kept: Vec<T> = Vec::new();
    for wanted in names {
        if let Some(held) = found
            .iter()
            .find(|held| name_of(held).eq_ignore_ascii_case(wanted))
            // The same vault named twice, in two spellings, is read once.
            && !kept.iter().any(|already| name_of(already) == name_of(held))
        {
            kept.push(held.clone());
        }
    }
    kept
}

/// The names in `names` that nothing in `found` answers to.
pub(crate) fn missing<T>(
    found: &[T],
    names: &[String],
    name_of: impl Fn(&T) -> &str,
) -> Vec<String> {
    names
        .iter()
        .map(|name| name.trim())
        .filter(|wanted| {
            !wanted.is_empty()
                && !found
                    .iter()
                    .any(|held| name_of(held).eq_ignore_ascii_case(wanted))
        })
        .map(str::to_owned)
        .collect()
}

/// What `narrow` calls a kind of resource: the noun, the `[azure]` key, the
/// flag, and the domain's doctor.
#[derive(Clone, Copy)]
pub(crate) struct Kind {
    pub noun: &'static str,
    pub key: &'static str,
    pub flag: &'static str,
    pub domain: &'static str,
}

/// The resources a command reads: the `[azure]` allowlist first, then the
/// command's own flag within it. A flag naming something the login cannot
/// reach, or the allowlist leaves out, is a usage error that lists what is
/// there; an allowlist that reaches nothing is a setup error, never an empty
/// answer.
pub(crate) fn narrow<T: Clone>(
    found: &[T],
    configured: &[String],
    only: &[String],
    kind: Kind,
    name_of: impl Fn(&T) -> &str + Copy,
) -> Result<Vec<T>> {
    let reachable = allowed(found, configured, name_of);
    let names = |items: &[T]| {
        let names: Vec<&str> = items.iter().map(name_of).collect();
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(", ")
        }
    };
    if reachable.is_empty() && configured.iter().any(|name| !name.trim().is_empty()) {
        return Err(Failure::setup(format!(
            "the login reaches none of [azure] {} ({})",
            kind.key,
            configured.join(", ")
        ))
        .hint(format!(
            "agent-cli doctor {} shows what the login reaches",
            kind.domain
        ))
        .into());
    }
    let gone = missing(&reachable, only, name_of);
    if let Some(name) = gone.first() {
        let why = if missing(found, only, name_of).contains(name) {
            "the login reaches no"
        } else {
            "[azure] leaves out the"
        };
        return Err(Failure::usage(format!(
            "{why} {} {name}; {} takes one of: {}",
            kind.noun,
            kind.flag,
            names(&reachable)
        ))
        .into());
    }
    Ok(allowed(&reachable, only, name_of))
}

/// Runs `read` over `items` on up to `limit` threads, one result per item in
/// the items' order. Ported from az-tui's pool: scoped threads and one
/// counter, which is all a bounded walk over a slice needs.
pub(crate) fn parallel<T: Sync, R: Send>(
    items: &[T],
    limit: usize,
    read: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    let work = || {
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(item) = items.get(index) else { break };
            let result = read(item);
            results
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())[index] = Some(result);
        }
    };
    std::thread::scope(|scope| {
        for _ in 1..limit.clamp(1, items.len().max(1)) {
            scope.spawn(work);
        }
        work();
    });
    results
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .into_iter()
        .flatten()
        .collect()
}

/// The first `limit` rows, with a note when there were more.
pub(crate) fn limited<T>(ctx: &Ctx, mut rows: Vec<T>, limit: usize) -> Vec<T> {
    if rows.len() > limit {
        ctx.note(format!("[{limit} of {}; --limit N]", rows.len()));
        rows.truncate(limit);
    }
    rows
}

/// A unix second, as a vault writes its attributes, in RFC 3339 UTC. `null`
/// is not 1970.
pub(crate) fn from_unix(value: &serde_json::Value) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(value.as_i64()?)
        .ok()
        .map(agent_cli_core::utc_time)
}

/// An RFC 3339 stamp as a registry writes it, in UTC whole seconds.
pub(crate) fn stamp(value: &serde_json::Value) -> Option<String> {
    parse_stamp(value.as_str()?).map(agent_cli_core::utc_time)
}

pub(crate) fn parse_stamp(raw: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(raw.trim(), &Rfc3339)
        .ok()
        .map(|instant| instant.to_offset(time::UtcOffset::UTC))
}

/// One string that was actually there: trimmed, and blank counts as absent.
pub(crate) fn text(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|held| !held.is_empty())
        .map(str::to_owned)
}

/// The HTTP status a refusal carried.
pub(crate) fn refused_with(error: &anyhow::Error) -> Option<u16> {
    agent_cli_core::status_of(error)
}

/// The first line a tool wrote that says anything.
pub(crate) fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_allowlist_fixes_the_order_and_ignores_case() {
        let found = vec![
            "kv-dev".to_owned(),
            "kv-prod".to_owned(),
            "kv-qa".to_owned(),
        ];
        let names = vec![
            "KV-PROD".to_owned(),
            " kv-dev ".to_owned(),
            "kv-gone".to_owned(),
            "kv-dev".to_owned(),
        ];
        assert_eq!(
            allowed(&found, &names, String::as_str),
            ["kv-prod", "kv-dev"]
        );
        assert_eq!(missing(&found, &names, String::as_str), ["kv-gone"]);
        assert_eq!(allowed(&found, &[" ".to_owned()], String::as_str), found);
    }

    #[test]
    fn narrowing_names_what_is_there_and_an_allowlist_reaching_nothing_is_setup() {
        let kind = Kind {
            noun: "vault",
            key: "vaults",
            flag: "--vault",
            domain: "kv",
        };
        let found = vec!["kv-dev".to_owned(), "kv-prod".to_owned()];
        let strings = |names: &[&str]| names.iter().map(|&n| n.to_owned()).collect::<Vec<_>>();
        let narrowed = narrow(&found, &strings(&["kv-prod"]), &[], kind, String::as_str).unwrap();
        assert_eq!(narrowed, ["kv-prod"]);

        let error = narrow(
            &found,
            &strings(&["kv-prod"]),
            &strings(&["kv-dev"]),
            kind,
            String::as_str,
        )
        .unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.exit, agent_cli_core::Exit::Usage);
        assert_eq!(
            failure.message,
            "[azure] leaves out the vault kv-dev; --vault takes one of: kv-prod"
        );

        let error = narrow(&found, &[], &strings(&["kv-x"]), kind, String::as_str).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("the login reaches no vault kv-x"),
            "{error}"
        );

        let error = narrow(&found, &strings(&["kv-gone"]), &[], kind, String::as_str).unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.exit, agent_cli_core::Exit::Setup);
        assert!(failure.hint.as_deref().unwrap().contains("doctor kv"));
    }

    #[test]
    fn the_pool_keeps_the_items_order_on_any_number_of_threads() {
        let items: Vec<usize> = (0..50).collect();
        for limit in [1, 4, 100] {
            assert_eq!(
                parallel(&items, limit, |n| n * 2),
                (0..50).map(|n| n * 2).collect::<Vec<_>>()
            );
        }
        assert!(parallel(&Vec::<usize>::new(), 4, |n| *n).is_empty());
    }

    #[test]
    fn stamps_come_out_as_rfc_3339_in_utc_and_null_is_not_1970() {
        assert_eq!(
            from_unix(&serde_json::json!(1_709_251_200)).as_deref(),
            Some("2024-03-01T00:00:00Z")
        );
        assert_eq!(from_unix(&serde_json::Value::Null), None);
        assert_eq!(
            stamp(&serde_json::json!("2026-09-11T15:00:00-05:00")).as_deref(),
            Some("2026-09-11T20:00:00Z")
        );
        assert_eq!(stamp(&serde_json::json!("yesterday")), None);
    }
}
