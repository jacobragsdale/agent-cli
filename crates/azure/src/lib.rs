//! The Azure domains of agent-cli, ported from az-tui: Key Vault secrets
//! (`kv`), container registry images (`acr`) and AKS clusters (`aks`).
//!
//! There is no Azure SDK and no credential of our own. Each plane gets an
//! `az` token for its own audience, and every call is a plain HTTPS request
//! through core's transport, which already carries az-tui's retry policy. The
//! guardrails az-tui kept are kept here:
//!
//! - a token goes only to its own hosts (`management.azure.com`,
//!   `*.vault.azure.net`, `*.azurecr.io`), checked on every URL, `nextLink`s
//!   and cached addresses included;
//! - an allowlist that reaches nothing is an error, never an empty answer;
//! - a name found in two vaults or registries is exit 2, never a guess;
//! - only `kv secret get` returns a value, and no row type has a field for one.

mod acr;
mod aks;
mod graph;
mod kv;

use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent_cli_core::{Check, Ctx, Domain, Failure, Secret};
use anyhow::Result;
use serde::{Deserialize, Deserializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const KV: Domain = Domain {
    name: "kv",
    summary: "Key Vault",
    commands: &[
        kv::SECRET_LIST,
        kv::SECRET_GET,
        kv::VERSION_LIST,
        kv::VAULT_LIST,
    ],
    synonyms: &[
        ("password", &["secret"]),
        ("passwords", &["secret"]),
        ("credential", &["secret"]),
        ("connection string", &["secret"]),
        ("api key", &["secret"]),
        ("keyvault", &["kv", "vault"]),
        ("key vault", &["kv", "vault"]),
        ("expire", &["expires"]),
        ("expiring", &["expires"]),
        ("rotated", &["version"]),
        ("history", &["version"]),
    ],
    status: kv::status,
    doctor: kv::doctor,
};

pub const ACR: Domain = Domain {
    name: "acr",
    summary: "Container Registry",
    commands: &[
        acr::REPO_LIST,
        acr::TAG_LIST,
        acr::MANIFEST_GET,
        acr::REGISTRY_LIST,
    ],
    synonyms: &[
        ("image", &["repo", "tag"]),
        ("images", &["repo", "tag"]),
        ("container image", &["repo", "tag", "manifest"]),
        ("docker", &["acr", "repo"]),
        ("repository", &["repo"]),
        ("repositories", &["repo"]),
        ("digest", &["manifest"]),
        ("sha256", &["manifest"]),
        ("architecture", &["manifest"]),
        ("pushed", &["tag", "updated"]),
    ],
    status: acr::status,
    doctor: acr::doctor,
};

pub const AKS: Domain = Domain {
    name: "aks",
    summary: "AKS",
    commands: &[aks::CLUSTER_LIST, aks::CLUSTER_CONNECT],
    synonyms: &[
        ("kubeconfig", &["connect"]),
        ("credentials", &["connect"]),
        ("get-credentials", &["connect"]),
        ("kubernetes cluster", &["aks", "cluster"]),
        ("clusters", &["cluster"]),
    ],
    status: aks::status,
    doctor: aks::doctor,
};

/// The ARM audience. The trailing slash is part of it: ARM refuses a token
/// minted for the same URL without one.
pub(crate) const ARM: &str = "https://management.azure.com/";
pub(crate) const VAULT: &str = "https://vault.azure.net";
/// What `az acr` itself asks for. Not ARM's: a hardened registry turns off
/// `azureADAuthenticationAsArmPolicy` and refuses an ARM-scoped token.
pub(crate) const REGISTRY: &str = "https://containerregistry.azure.net";

/// The `[azure]` section. An empty list means everything the login reaches;
/// a non-empty one is an allowlist that also fixes the order rows come in.
///
/// ```toml
/// [azure]
/// subscriptions = ["00000000-0000-0000-0000-000000000000"]  # or one string
/// vaults = ["kv-contoso-dev", "kv-contoso-prod"]
/// registries = "contosoacr"
/// refresh = 300    # seconds the Resource Graph inventory and ACR attributes are cached
/// parallel = 8     # vaults, registries or repositories read at once
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Azure {
    #[serde(deserialize_with = "one_or_many")]
    pub subscriptions: Vec<String>,
    #[serde(deserialize_with = "one_or_many")]
    pub vaults: Vec<String>,
    #[serde(deserialize_with = "one_or_many")]
    pub registries: Vec<String>,
    pub refresh: Option<u64>,
    pub parallel: Option<usize>,
}

impl Azure {
    pub fn load(ctx: &Ctx) -> Result<Self> {
        ctx.section("azure")
    }

    /// How long the inventory and registry attributes stay cached. Zero turns
    /// the cache off.
    pub fn refresh(&self) -> Duration {
        Duration::from_secs(self.refresh.unwrap_or(300))
    }

    /// Eight is well inside every plane's per-resource quota; a Basic-tier
    /// registry with hundreds of repositories is where to turn it down.
    pub fn parallel(&self) -> usize {
        self.parallel.unwrap_or(8).max(1)
    }
}

/// A key that takes one string or a list of them.
pub(crate) fn one_or_many<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(one) => vec![one],
        OneOrMany::Many(many) => many,
    })
}

/// The status line for a domain whose config is an allowlist in `[azure]`.
fn allowlist_status(
    config: &agent_cli_core::Config,
    domain: &str,
    noun: &str,
    names: fn(&Azure) -> &[String],
) -> String {
    match config.section::<Azure>("azure") {
        Ok(azure) if names(&azure).is_empty() => format!("{domain} all {noun}"),
        Ok(azure) => format!("{domain} {} {noun}", names(&azure).len()),
        Err(_) => format!("{domain} config broken"),
    }
}

/// The checks every Azure domain starts with: `[azure]` parses, `az` hands
/// out an ARM token (so it is installed and signed in), then a token for each
/// of `audiences`. `None` when the rest cannot run.
pub(crate) fn doctor_login(
    ctx: &Ctx,
    checks: &mut Vec<Check>,
    audiences: &[(&str, &str)],
) -> Option<Azure> {
    let azure = match Azure::load(ctx) {
        Ok(azure) => azure,
        Err(error) => {
            checks.push(Check::failed(
                "config",
                format!("{error:#}"),
                "fix [azure]; config.example.toml shows every key",
            ));
            return None;
        }
    };
    for &(check, resource) in [("az login", ARM)].iter().chain(audiences) {
        let started = std::time::Instant::now();
        match az_token(ctx, resource, false) {
            Ok(_) => checks.push(Check::ok(
                check,
                format!(
                    "token for {resource} in {} ms",
                    started.elapsed().as_millis()
                ),
            )),
            Err(error) => {
                checks.push(Check::failed(check, format!("{error:#}"), "az login"));
                return None;
            }
        }
    }
    Some(azure)
}

/// `az`'s token for `resource`: the real one, or in tests a fixed one, since
/// `az` is not signed in where the tests run.
#[cfg(not(test))]
pub(crate) fn az_token(ctx: &Ctx, resource: &str, fresh: bool) -> Result<Secret> {
    ctx.az_token(resource, fresh)
}

#[cfg(test)]
pub(crate) fn az_token(_: &Ctx, resource: &str, fresh: bool) -> Result<Secret> {
    let audience = match resource {
        ARM => "arm",
        VAULT => "vault",
        _ => "registry",
    };
    let generation = if fresh { "fresh" } else { "first" };
    Ok(Secret::new(format!("{audience}-token-{generation}")))
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

/// A window such as `30d`, `12h`, `2w` or `90m`; a bad one is a usage error
/// naming the flag.
pub(crate) fn window(flag: &str, raw: &str) -> Result<Duration> {
    let raw = raw.trim();
    let split = raw.find(|c: char| !c.is_ascii_digit()).unwrap_or(raw.len());
    let (count, unit) = raw.split_at(split);
    let unit = match unit {
        "m" => 60,
        "h" => 3600,
        "d" | "" => 86_400,
        "w" => 7 * 86_400,
        _ => 0,
    };
    match count.parse::<u64>() {
        Ok(count) if unit > 0 => Ok(Duration::from_secs(count * unit)),
        _ => Err(Failure::usage(format!(
            "{flag} takes a window like 30d, 12h, 2w or 90m, not {raw:?}"
        ))
        .into()),
    }
}

/// A unix second, as a vault writes its attributes, in RFC 3339. `null` is not
/// 1970.
pub(crate) fn from_unix(value: &serde_json::Value) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(value.as_i64()?)
        .ok()?
        .format(&Rfc3339)
        .ok()
}

/// An RFC 3339 stamp as a registry writes it, normalised to UTC.
pub(crate) fn stamp(value: &serde_json::Value) -> Option<String> {
    parse_stamp(value.as_str()?)?.format(&Rfc3339).ok()
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

/// The HTTP status a refusal carried, read back from core's
/// `"<METHOD> <url> answered <status>: …"` failure message.
pub(crate) fn refused_with(error: &anyhow::Error) -> Option<u16> {
    let failure = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<Failure>())?;
    let (_, rest) = failure.message.split_once(" answered ")?;
    rest.get(..3)?.parse().ok()
}

/// The first line a tool wrote that says anything.
pub(crate) fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Scrubbed answers the kv, acr and aks tests share.

    use agent_cli_core::testing::{Answer, FakeTransport, Outcome, run};
    use agent_cli_core::{Domain, Setup};
    use serde_json::{Value, json};

    /// One thread, so the fake's answers land in the order they were given.
    pub const SERIAL: &str = "[azure]\nparallel = 1\n";

    pub fn azure(
        domains: &[Domain],
        argv: &[&str],
        answers: Vec<Answer>,
    ) -> (Outcome, FakeTransport) {
        azure_with(domains, argv, answers, SERIAL)
    }

    pub fn azure_with(
        domains: &[Domain],
        argv: &[&str],
        answers: Vec<Answer>,
        config: &str,
    ) -> (Outcome, FakeTransport) {
        let transport = FakeTransport::answering(answers);
        let outcome = run(
            domains,
            argv,
            Setup::fake(transport.clone()).with_config(config),
        );
        (outcome, transport)
    }

    fn row(kind: &str, name: &str, extra: Value) -> Value {
        let mut row = json!({
            "id": format!("/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-contoso/providers/{kind}/{name}"),
            "name": name,
            "type": kind.to_ascii_lowercase(),
            "subscriptionId": "00000000-0000-0000-0000-000000000001",
            "resourceGroup": "rg-contoso",
            "location": "eastus",
            "loginServer": "",
            "vaultUri": "",
            "kubernetesVersion": "",
            "currentKubernetesVersion": "",
            "powerState": "",
        });
        for (key, value) in extra.as_object().into_iter().flatten() {
            row[key] = value.clone();
        }
        row
    }

    pub fn vault(name: &str) -> Value {
        row(
            "Microsoft.KeyVault/vaults",
            name,
            json!({"vaultUri": format!("https://{name}.vault.azure.net/")}),
        )
    }

    pub fn registry(name: &str) -> Value {
        row(
            "Microsoft.ContainerRegistry/registries",
            name,
            json!({"loginServer": format!("{name}.azurecr.io")}),
        )
    }

    pub fn cluster(name: &str) -> Value {
        row(
            "Microsoft.ContainerService/managedClusters",
            name,
            json!({"currentKubernetesVersion": "1.30.4", "kubernetesVersion": "1.30", "powerState": "Running"}),
        )
    }

    /// A context for calling a domain's doctor directly.
    pub fn doctor_ctx(answers: Vec<Answer>, config: &str) -> (agent_cli_core::Ctx, FakeTransport) {
        let transport = FakeTransport::answering(answers);
        let ctx = agent_cli_core::testing::ctx(Setup::fake(transport.clone()).with_config(config));
        (ctx, transport)
    }

    /// Each check's name and whether it passed.
    pub fn rows(checks: &[agent_cli_core::Check]) -> Vec<(String, bool)> {
        checks
            .iter()
            .map(|check| (check.check.clone(), check.ok))
            .collect()
    }

    /// A Resource Graph answer with `rows`, on one page.
    pub fn inventory(rows: Vec<Value>) -> Answer {
        Answer::json(&json!({"totalRecords": rows.len(), "count": rows.len(), "data": rows}))
    }
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
    fn a_window_reads_minutes_to_weeks_and_a_bad_one_is_a_usage_error() {
        assert_eq!(
            window("--x", "30d").unwrap(),
            Duration::from_secs(30 * 86_400)
        );
        assert_eq!(
            window("--x", "12h").unwrap(),
            Duration::from_secs(12 * 3600)
        );
        assert_eq!(
            window("--x", "2w").unwrap(),
            Duration::from_secs(14 * 86_400)
        );
        assert_eq!(window("--x", "7").unwrap(), Duration::from_secs(7 * 86_400));
        for bad in ["", "d", "soon", "3y", "-1d"] {
            let error = window("--expires-within", bad).unwrap_err();
            assert!(
                error.to_string().starts_with("--expires-within takes"),
                "{bad}"
            );
        }
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

    #[test]
    fn the_status_of_a_refusal_is_read_back_from_cores_message() {
        let error = anyhow::Error::new(Failure::new(
            agent_cli_core::Exit::Failed,
            "GET https://kv.vault.azure.net/x answered 403: nope",
        ));
        assert_eq!(refused_with(&error), Some(403));
        assert_eq!(refused_with(&anyhow::anyhow!("socket closed")), None);
    }
}
