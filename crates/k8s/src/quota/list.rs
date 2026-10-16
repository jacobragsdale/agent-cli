use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, items, limited, non_empty};
use crate::pod::strings;

#[derive(clap::Args)]
pub struct QuotaListArgs {
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// One resource a ResourceQuota bounds, or one a LimitRange sizes.
#[derive(Debug, Serialize, JsonSchema)]
pub struct QuotaRow {
    namespace: String,
    /// ResourceQuota/NAME or LimitRange/NAME.
    from: String,
    /// requests.cpu, pods, memory…
    resource: String,
    /// A quota's: what the namespace's objects take now.
    used: Option<String>,
    /// A quota's bound: a pod that would pass it is refused.
    hard: Option<String>,
    /// A limit range's: Container, Pod or PersistentVolumeClaim.
    applies_to: Option<String>,
    /// The limit a container that sets none gets.
    default: Option<String>,
    /// The request a container that sets none gets.
    default_request: Option<String>,
    min: Option<String>,
    max: Option<String>,
}

fn quota_list(ctx: &Ctx, args: QuotaListArgs) -> Result<Vec<QuotaRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "resourcequotas,limitranges", "-o", "json"])?;
    let mut rows = Vec::new();
    for item in items(&listed) {
        let namespace = item["metadata"]["namespace"].as_str().unwrap_or_default();
        let from = format!(
            "{}/{}",
            item["kind"].as_str().unwrap_or_default(),
            item["metadata"]["name"].as_str().unwrap_or_default()
        );
        let row = |resource: String, applies_to: Option<&str>| QuotaRow {
            namespace: namespace.to_owned(),
            from: from.clone(),
            resource,
            used: None,
            hard: None,
            applies_to: applies_to.map(str::to_owned),
            default: None,
            default_request: None,
            min: None,
            max: None,
        };
        if item["kind"] == "ResourceQuota" {
            let used = &item["status"]["used"];
            rows.extend(
                strings(&item["status"]["hard"])
                    .into_iter()
                    .map(|(resource, hard)| QuotaRow {
                        used: Some(used[&resource].as_str().unwrap_or("0").to_owned()),
                        hard: Some(hard),
                        ..row(resource, None)
                    }),
            );
            continue;
        }
        for limit in item["spec"]["limits"].as_array().into_iter().flatten() {
            let kind = non_empty(&limit["type"]);
            let sized =
                |key: &str, resource: &str| non_empty(&limit[key][resource]).map(str::to_owned);
            let mut resources: Vec<String> = ["default", "defaultRequest", "min", "max"]
                .iter()
                .flat_map(|key| strings(&limit[*key]).into_keys())
                .collect();
            resources.sort();
            resources.dedup();
            rows.extend(resources.into_iter().map(|resource| QuotaRow {
                default: sized("default", &resource),
                default_request: sized("defaultRequest", &resource),
                min: sized("min", &resource),
                max: sized("max", &resource),
                ..row(resource, kind)
            }));
        }
    }
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub QUOTA_LIST = ["k8s", "quota", "list"], Read,
    "List resource quotas used of hard, and limit ranges' default requests and limits",
    keywords: ["resourcequota", "limitrange", "exceeded", "forbidden", "defaults", "limits", "requests", "namespace"],
    example: "k8s quota list --cluster prod --namespace web --fields from,resource,used,hard",
    run: quota_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn quotas_list_used_of_hard_and_limit_ranges_their_defaults() {
        let outcome = k8s(&["k8s", "quota", "list", "--cluster", "all"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"namespace": "dev", "from": "ResourceQuota/compute", "resource": "pods", "used": "5", "hard": "20"},
                {"namespace": "dev", "from": "ResourceQuota/compute", "resource": "requests.cpu", "used": "250m", "hard": "4"},
                {"namespace": "dev", "from": "ResourceQuota/compute", "resource": "requests.memory", "used": "256Mi", "hard": "8Gi"},
                {"namespace": "qa", "from": "LimitRange/defaults", "resource": "cpu", "applies_to": "Container", "default_request": "50m"},
                {"namespace": "qa", "from": "LimitRange/defaults", "resource": "memory", "applies_to": "Container", "default": "256Mi", "default_request": "64Mi"},
                {"namespace": "qa", "from": "LimitRange/defaults", "resource": "storage", "applies_to": "PersistentVolumeClaim", "max": "50Gi"},
            ])
        );
    }
}
