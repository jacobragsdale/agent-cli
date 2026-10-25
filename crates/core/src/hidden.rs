//! What a person cannot use stays out of search and the overview: domains
//! not set up here, and domains the last `doctor` found refused (a 403).
//!
//! A team build carries every domain, and a teammate without a cluster or
//! a role should not see an agent reach for `k8s` or `entra` first. Their
//! commands still run (and say what is missing); `search --all` still
//! finds them.

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::Config;
use crate::doctor::Check;
use crate::registry::Domain;

/// Why a domain is hidden.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Why {
    NotSetUp,
    Denied,
}

impl Why {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::NotSetUp => "not set up",
            Self::Denied => "denied",
        }
    }
}

/// The hidden domains, in registry order. A domain is not set up when its
/// status line says so (every domain's `status` ends in `not set up` then).
pub(crate) fn hidden(
    domains: &[Domain],
    config: &Config,
    state_dir: Option<&Path>,
) -> Vec<(&'static str, Why)> {
    let denied = state_dir.map(load).unwrap_or_default();
    domains
        .iter()
        .filter_map(|domain| {
            if (domain.status)(config).trim_end().ends_with("not set up") {
                Some((domain.name, Why::NotSetUp))
            } else if denied.contains_key(domain.name) {
                Some((domain.name, Why::Denied))
            } else {
                None
            }
        })
        .collect()
}

/// The overview of the domains not `hidden`, and one line folding those.
pub(crate) fn overview(domains: &[Domain], config: &Config, hidden: &[(&str, Why)]) -> String {
    let shown: Vec<&Domain> = domains
        .iter()
        .filter(|domain| !hidden.iter().any(|(name, _)| *name == domain.name))
        .collect();
    let text = crate::discover::overview(&shown, config);
    let mut folded = Vec::new();
    for why in [Why::NotSetUp, Why::Denied] {
        let names: Vec<&str> = (hidden.iter())
            .filter(|(_, said)| *said == why)
            .map(|(name, _)| *name)
            .collect();
        if !names.is_empty() {
            folded.push(format!("{} ({})", names.join(" "), why.label()));
        }
    }
    if folded.is_empty() {
        return text;
    }
    format!(
        "{text}\nHidden:      {}; agent-cli search --all WORDS",
        folded.join(" \u{b7} ")
    )
}

/// Rows that only say a domain is configured and signed in: they pass for a
/// person whose login has no role, so they do not count against denied.
fn is_setup_check(name: &str) -> bool {
    ["config", "credential", "az login", "kubectl", "kubelogin"].contains(&name)
        || name.ends_with(" token")
        || name.ends_with(" credential")
}

/// A domain doctor found refused: a denied check, and nothing beyond the
/// config and credential rows passing. Its first denied detail, if so.
pub(crate) fn denied_detail(checks: &[Check]) -> Option<String> {
    let reached = checks
        .iter()
        .any(|check| check.ok && !is_setup_check(&check.check));
    if reached {
        return None;
    }
    checks
        .iter()
        .find(|check| check.denied)
        .map(|check| check.detail.clone())
}

/// `denied.json`: domain to the detail doctor saw.
fn load(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read(dir.join("denied.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Records what doctor found for each domain it checked: denied ones are
/// kept, the rest cleared; domains it did not check keep their entry.
pub(crate) fn record(dir: &Path, checked: &[(&str, Option<String>)]) {
    let mut denied = load(dir);
    for (domain, detail) in checked {
        match detail {
            Some(detail) => denied.insert((*domain).to_owned(), detail.clone()),
            None => denied.remove(*domain),
        };
    }
    let Ok(text) = serde_json::to_string_pretty(&denied) else {
        return;
    };
    let _ = std::fs::create_dir_all(dir).and_then(|()| {
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        file.persist(dir.join("denied.json"))
            .map(drop)
            .map_err(|error| error.error)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_is_denied_only_when_nothing_past_sign_in_passed() {
        let refused =
            Check::failed("vault kv-contoso", "GET … answered 403: Forbidden", "ask").denied();
        let denied = [
            Check::ok("az login", "me"),
            Check::ok("vault token", "ok"),
            refused.clone(),
        ];
        assert!(denied_detail(&denied).unwrap().contains("403"));
        let partly = [
            Check::ok("az login", "me"),
            Check::ok("inventory", "2"),
            refused,
        ];
        assert_eq!(denied_detail(&partly), None);
        assert_eq!(
            denied_detail(&[Check::failed("connection", "down", "h")]),
            None
        );
    }

    #[test]
    fn doctor_records_denied_domains_and_a_passing_run_clears_them() {
        let dir = tempfile::tempdir().unwrap();
        record(
            dir.path(),
            &[("entra", Some("403".into())), ("net", Some("403".into()))],
        );
        record(dir.path(), &[("net", None)]);
        let kept = load(dir.path());
        assert_eq!(kept.keys().collect::<Vec<_>>(), ["entra"]);
    }
}
