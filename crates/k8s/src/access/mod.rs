//! `k8s access …`: what RBAC lets a subject do in a namespace, from
//! `kubectl auth can-i --list`, which `serviceaccount get` shows too.

pub(crate) mod list;

use agent_cli_core::Ctx;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::Target;

#[derive(Debug, Serialize, JsonSchema)]
pub struct AccessRow {
    /// pods, deployments.apps, `*.*` for every kind.
    resource: String,
    /// Only these objects of it; empty is every one.
    names: Vec<String>,
    /// get, list, watch, create… `*` for every verb.
    verbs: Vec<String>,
}

/// What `subject` (yourself when None) may do in the target's namespace.
pub(crate) fn rules(ctx: &Ctx, target: &Target, subject: Option<&str>) -> Result<Vec<AccessRow>> {
    let mut argv = vec!["auth", "can-i", "--list", "--no-headers"];
    if let Some(subject) = subject {
        argv.extend(["--as", subject]);
    }
    Ok(target.read(ctx, &argv)?.lines().filter_map(rule).collect())
}

/// One line of `auth can-i --list`: `RESOURCE  [URLS]  [NAMES]  [VERBS]`.
/// Non-resource URLs (`/healthz`, `/api/*`) and the self-reviews every
/// signed-in subject may make say nothing about this one, so they are left
/// out.
fn rule(line: &str) -> Option<AccessRow> {
    let (resource, rest) = line.split_at(line.find('[')?);
    let lists: Vec<Vec<String>> = rest
        .split(']')
        .filter_map(|held| held.trim().strip_prefix('['))
        .map(|held| held.split_whitespace().map(str::to_owned).collect())
        .collect();
    let [_, names, verbs] = lists.as_slice() else {
        return None;
    };
    let resource = resource.trim();
    (!resource.is_empty() && !resource.starts_with("selfsubject")).then(|| AccessRow {
        resource: resource.to_owned(),
        names: names.clone(),
        verbs: verbs.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_reads_as_its_resource_names_and_verbs_and_urls_are_left_out() {
        let row = rule("secrets      []   [worker-db orders-db]   [get list]").unwrap();
        assert_eq!(row.resource, "secrets");
        assert_eq!(row.names, ["worker-db", "orders-db"]);
        assert_eq!(row.verbs, ["get", "list"]);
        assert!(rule("             [/healthz]   []   [get]").is_none());
        assert!(rule("selfsubjectreviews.authentication.k8s.io   []   []   [create]").is_none());
        assert!(rule("no brackets").is_none());
    }
}
