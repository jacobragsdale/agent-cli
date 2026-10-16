use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, age, items, limited, name_has, non_empty, scopes};
use crate::pod::strings;

#[derive(clap::Args)]
pub struct NamespaceListArgs {
    /// Part of the namespace's name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NamespaceRow {
    name: String,
    /// Active, or Terminating while its objects are deleted.
    status: Option<String>,
    labels: BTreeMap<String, String>,
    /// Each resource its quotas bound, as used/hard: `{"requests.cpu": "6500m/12"}`.
    quota: BTreeMap<String, String>,
    /// Whether the [[k8s.scope]] takes it as --namespace.
    in_scope: bool,
    age: Option<String>,
}

/// The label every namespace carries, its own name again.
const NAME_LABEL: &str = "kubernetes.io/metadata.name";

fn namespace_list(ctx: &Ctx, args: NamespaceListArgs) -> Result<Vec<NamespaceRow>> {
    let target = args.at.cluster_wide(ctx)?;
    let allowed = scopes(ctx.config())?
        .into_iter()
        .find(|scope| scope.name == target.scope)
        .map(|scope| scope.namespaces)
        .unwrap_or_default();
    let listed = target.json(
        ctx,
        &[
            "get",
            "namespaces,resourcequotas",
            "-o",
            "json",
            "--all-namespaces",
        ],
    )?;
    let (namespaces, quotas): (Vec<&Value>, Vec<&Value>) =
        items(&listed).partition(|item| item["kind"] == "Namespace");
    let rows = namespaces
        .into_iter()
        .filter(|item| name_has(item, args.name.as_deref()))
        .filter_map(|item| {
            let name = non_empty(&item["metadata"]["name"])?;
            let mut labels = strings(&item["metadata"]["labels"]);
            labels.remove(NAME_LABEL);
            Some(NamespaceRow {
                name: name.to_owned(),
                status: non_empty(&item["status"]["phase"]).map(str::to_owned),
                labels,
                quota: quota(&quotas, name),
                in_scope: allowed.is_empty() || allowed.iter().any(|held| held == name),
                age: age(&item["metadata"]["creationTimestamp"]),
            })
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

/// A namespace's quotas, resource by resource.
// ponytail: two quotas bounding one resource show the later one's; `quota
// list` shows each.
fn quota(quotas: &[&Value], namespace: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for held in quotas
        .iter()
        .filter(|held| held["metadata"]["namespace"] == namespace)
    {
        let used = &held["status"]["used"];
        for (resource, hard) in strings(&held["status"]["hard"]) {
            let used = used[&resource].as_str().unwrap_or("0");
            found.insert(resource, format!("{used}/{hard}"));
        }
    }
    found
}

command! {
    pub NAMESPACE_LIST = ["k8s", "namespace", "list"], Read,
    "List namespaces: status, labels, quota used of hard, whether the scope reads it",
    keywords: ["namespaces", "projects", "terminating", "scope", "cluster", "read"],
    example: "k8s namespace list --cluster prod --fields name,status,quota,in_scope",
    run: namespace_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn namespaces_show_their_quota_and_whether_the_scope_reads_them() {
        let outcome = k8s(&[
            "k8s",
            "namespace",
            "list",
            "--cluster",
            "qa",
            "--fields",
            "name,labels,quota,in_scope",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"name": "dev", "labels": {"team": "orders"},
                    "quota": {"pods": "5/20", "requests.cpu": "250m/4", "requests.memory": "256Mi/8Gi"},
                    "in_scope": true},
                {"name": "default", "in_scope": false},
                {"name": "kube-system", "in_scope": false},
                {"name": "qa", "in_scope": true},
                {"name": "uat", "in_scope": true},
            ])
        );
        let all = k8s(&[
            "k8s",
            "namespace",
            "list",
            "--cluster",
            "all",
            "--fields",
            "in_scope",
        ]);
        assert!(
            all.json()
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["in_scope"] == true),
            "a scope that names none reads every namespace"
        );
    }
}
