//! `k8s kind`: what the API server serves, read from `kubectl api-resources`
//! and cached, because `k8s object` must know whether a kind is namespaced
//! before it can scope the call or shape an id.

pub(crate) mod list;

use std::time::Duration;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::kubectl::Target;

/// A cluster gains a kind when someone installs a CRD, which is rare; a
/// kind missing from the cache is read again before it is called unknown.
const KINDS_TTL: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct KindRow {
    /// The plural resource name kubectl takes: pods, certificates.
    pub(crate) name: String,
    pub(crate) short_names: Vec<String>,
    /// group/version: v1, apps/v1, cert-manager.io/v1.
    pub(crate) api_version: String,
    /// False for cluster-scoped kinds, whose ids are cluster/name.
    pub(crate) namespaced: bool,
    pub(crate) kind: String,
    pub(crate) verbs: Vec<String>,
}

impl KindRow {
    /// `deployments.apps`: unambiguous across API groups, as kubectl takes it.
    pub(crate) fn qualified(&self) -> String {
        match self.api_version.rsplit_once('/') {
            Some((group, _)) => format!("{}.{group}", self.name),
            None => self.name.clone(),
        }
    }

    /// Whether `word` names this kind as kubectl reads it: plural, singular,
    /// short name or kind, any case, with an optional `.group`.
    fn is(&self, word: &str) -> bool {
        let word = word.to_ascii_lowercase();
        let (base, group) = word.split_once('.').unwrap_or((&word, ""));
        let held_group = self
            .api_version
            .rsplit_once('/')
            .map_or("", |(group, _)| group);
        (base == self.name
            || base == self.kind.to_ascii_lowercase()
            || self.short_names.iter().any(|short| short == base))
            && (group.is_empty() || group == held_group)
    }
}

/// Every kind the cluster serves, from the cache when it is fresh.
pub(crate) fn kinds(ctx: &Ctx, target: &Target, fresh: bool) -> Result<Vec<KindRow>> {
    let key = format!("k8s/api-resources/{}", target.context);
    if !fresh && let Some(rows) = ctx.cache().get(&key) {
        return Ok(rows);
    }
    let rows = parse(&target.read(ctx, &["api-resources", "-o", "wide"])?);
    ctx.cache().put(&key, &rows, KINDS_TTL);
    Ok(rows)
}

/// The kind `word` names, read again when the cache does not hold it.
pub(crate) fn kind_named(ctx: &Ctx, target: &Target, word: &str) -> Result<KindRow> {
    for fresh in [false, true] {
        if let Some(found) = kinds(ctx, target, fresh)?
            .into_iter()
            .find(|row| row.is(word))
        {
            return Ok(found);
        }
    }
    Err(
        Failure::not_found(format!("cluster {} serves no kind {word:?}", target.scope))
            .hint(format!(
                "agent-cli k8s kind list --cluster {}",
                target.scope
            ))
            .into(),
    )
}

/// `kubectl api-resources -o wide`, a table with no JSON form. Columns are
/// cut where the header's words start: SHORTNAMES may be blank, and VERBS
/// holds spaces (`[get list watch]`).
fn parse(table: &str) -> Vec<KindRow> {
    let mut lines = table.lines();
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let starts: Vec<usize> = [
        "NAME",
        "SHORTNAMES",
        "APIVERSION",
        "NAMESPACED",
        "KIND",
        "VERBS",
    ]
    .iter()
    .filter_map(|column| header.find(column))
    .collect();
    if starts.len() != 6 {
        return Vec::new();
    }
    let categories = header.find("CATEGORIES").unwrap_or(usize::MAX);
    lines
        .filter_map(|line| {
            let cell = |i: usize| {
                let end = starts.get(i + 1).copied().unwrap_or(categories);
                line.get(starts[i].min(line.len())..end.min(line.len()))
                    .unwrap_or_default()
                    .trim()
            };
            let words = |cell: &str| -> Vec<String> {
                cell.trim_matches(['[', ']'])
                    .split([',', ' '])
                    .filter(|word| !word.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            (!cell(0).is_empty()).then(|| KindRow {
                name: cell(0).to_owned(),
                short_names: words(cell(1)),
                api_version: cell(2).to_owned(),
                namespaced: cell(3) == "true",
                kind: cell(4).to_owned(),
                verbs: words(cell(5)),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_resources_parse_by_the_header_with_blank_short_names_and_spaced_verbs() {
        let rows = parse(
            "NAME          SHORTNAMES   APIVERSION           NAMESPACED   KIND          VERBS                 CATEGORIES\n\
             secrets                    v1                   true         Secret        [get list]            \n\
             certificates  cert,certs   cert-manager.io/v1   true         Certificate   [get list watch]      cert-manager\n\
             nodes         no           v1                   false        Node          [get]\n",
        );
        assert_eq!(rows.len(), 3);
        assert!(rows[0].short_names.is_empty());
        assert_eq!(rows[1].short_names, ["cert", "certs"]);
        assert_eq!(rows[1].verbs, ["get", "list", "watch"]);
        assert_eq!(rows[1].qualified(), "certificates.cert-manager.io");
        assert!(!rows[2].namespaced);
        assert!(
            rows[1].is("Certificate")
                && rows[1].is("cert")
                && rows[1].is("certificates.cert-manager.io")
        );
        assert!(!rows[1].is("certificates.example.com"));
        assert!(rows[2].is("no") && rows[2].is("node"));
    }
}
