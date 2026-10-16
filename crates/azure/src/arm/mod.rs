//! What the arm commands share: the subscriptions the login reaches (the
//! scope every Resource Graph query runs in), and KQL string literals.
//!
//! Every row's id is an ARM id (a child collection too), which `arm resource
//! get` takes, as it takes a portal link or a name with `--type`, `--group`
//! and `--subscription` (`resource/mod.rs`, which net shares). Reads pick
//! the provider's newest stable api-version, cached (`resource/get.rs`).

pub(crate) mod group;
pub(crate) mod query;
pub(crate) mod resource;
pub(crate) mod subscription;

use agent_cli_core::{Ctx, Request, host_under, redact};
use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{ARM, Kind, bearer, narrow, text};
use crate::config::Azure;

pub(crate) const SUBSCRIPTIONS: Kind = Kind {
    noun: "subscription",
    key: "subscriptions",
    flag: "--subscription",
    domain: "arm",
};

/// The api-version `graph.rs` asks `/subscriptions` with; also what `arm
/// resource get` reads one subscription with.
pub(crate) const SUBSCRIPTIONS_API: &str = "2022-12-01";

/// One subscription.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct Subscription {
    /// `/subscriptions/GUID`, what `arm resource get` takes; `--subscription`
    /// takes it, the GUID or the name.
    pub id: String,
    pub name: String,
    /// Enabled, Warned, PastDue, Disabled or Deleted.
    pub state: String,
    pub tenant: String,
}

impl Subscription {
    /// The GUID, which `[azure] subscriptions` names.
    pub fn guid(&self) -> &str {
        self.id.trim_start_matches("/subscriptions/")
    }
}

/// Every subscription the login reaches, before the allowlist: ARM's
/// `/subscriptions`, every page, cached for `[azure] refresh`.
pub(crate) fn subscriptions(ctx: &Ctx, azure: &Azure) -> Result<Vec<Subscription>> {
    let key = "azure:subscriptions";
    if let Some(held) = ctx.cache().get::<Vec<Subscription>>(key) {
        return Ok(held);
    }
    let mint = bearer(ctx, ARM);
    let mut found = Vec::new();
    let mut url =
        format!("https://management.azure.com/subscriptions?api-version={SUBSCRIPTIONS_API}");
    loop {
        // A nextLink is ARM's to write; it gets the token only if it stays there.
        if !host_under(&url, "management.azure.com") {
            bail!("{url} is not ARM; the ARM token is not sent there");
        }
        let answer = ctx.read(Request::get(&url).auth(&mint))?.json()?;
        for held in answer["value"].as_array().into_iter().flatten() {
            let field = |key: &str| text(&held[key]).unwrap_or_default();
            found.push(Subscription {
                id: format!("/subscriptions/{}", field("subscriptionId")),
                name: field("displayName"),
                state: field("state"),
                tenant: field("tenantId"),
            });
        }
        match text(&answer["nextLink"]) {
            Some(next) if next != url => url = next,
            _ => break,
        }
    }
    ctx.cache().put(key, &found, azure.refresh());
    Ok(found)
}

/// The subscriptions a Resource Graph query runs over: `[azure]
/// subscriptions` (empty: every one the login reaches), or the one
/// `--subscription` names within it, by GUID, id or name.
pub(crate) fn scope(ctx: &Ctx, azure: &Azure, flag: Option<&str>) -> Result<Vec<String>> {
    let Some(flag) = flag else {
        return Ok(azure.subscriptions.clone());
    };
    let found = subscriptions(ctx, azure)?;
    let wanted = flag.trim().trim_start_matches("/subscriptions/");
    let guid = found
        .iter()
        .find(|held| {
            held.guid().eq_ignore_ascii_case(wanted) || held.name.eq_ignore_ascii_case(wanted)
        })
        .map_or(wanted, Subscription::guid)
        .to_owned();
    let kept = narrow(
        &found,
        &azure.subscriptions,
        &[guid],
        SUBSCRIPTIONS,
        Subscription::guid,
    )?;
    Ok(kept.iter().map(|held| held.guid().to_owned()).collect())
}

/// Any resource's JSON with secret-shaped text masked (a password in a
/// connection string, a SAS `sig=`, a bearer token). ARM keeps secrets behind
/// `list*` POSTs, but a GET or a query is any type's shape. Unlike
/// `redact_value`, fields named like secrets are kept: in ARM they hold
/// references (`keyVaultSecretId`, `secretName`), which are the join.
pub(crate) fn scrub(value: Value) -> Value {
    match value {
        Value::String(held) => Value::String(redact(&held)),
        Value::Array(items) => Value::Array(items.into_iter().map(scrub).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, held)| (key, scrub(held)))
                .collect(),
        ),
        other => other,
    }
}

/// `raw` as a KQL string literal: single-quoted, its quotes and backslashes
/// escaped, so a name never becomes query text.
pub(crate) fn quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// A KQL test that `type` is one of `kinds`, in any case.
pub(crate) fn of_type(kinds: &[&str]) -> String {
    match kinds {
        [one] => format!("type =~ {}", quote(one)),
        many => {
            let quoted: Vec<String> = many.iter().map(|kind| quote(kind)).collect();
            format!("type in~ ({})", quoted.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quote_in_a_name_stays_inside_the_literal() {
        assert_eq!(quote("kv-contoso"), "'kv-contoso'");
        assert_eq!(quote(r"a' or 1==1 \"), r"'a\' or 1==1 \\'");
    }
}
