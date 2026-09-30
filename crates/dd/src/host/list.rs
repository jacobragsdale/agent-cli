use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Dd, epoch, limited, strings, tag, text};

#[derive(clap::Args)]
pub struct HostListArgs {
    /// Host name or tag to match: 'aks-nodepool1', 'env:prod'
    query: Option<String>,
    /// kube_cluster_name tag
    #[arg(long)]
    cluster: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HostRow {
    /// The host name.
    id: String,
    up: Option<bool>,
    last_reported: Option<String>,
    apps: Vec<String>,
    /// Its kube_cluster_name tag.
    cluster: Option<String>,
    muted: Option<bool>,
}

fn host_list(ctx: &Ctx, args: HostListArgs) -> Result<Vec<HostRow>> {
    let dd = Dd::load(ctx)?;
    let mut filter: Vec<String> = args.query.iter().cloned().collect();
    filter.extend(
        args.cluster
            .map(|cluster| format!("kube_cluster_name:{cluster}")),
    );
    let mut query = vec![("count", args.limit.clamp(1, 1000).to_string())];
    if !filter.is_empty() {
        query.insert(0, ("filter", filter.join(" ")));
    }
    let found = dd.get(ctx, "/api/v1/hosts", &query)?;
    let rows: Vec<HostRow> = found["host_list"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|host| {
            let tags: Vec<Value> = host["tags_by_source"]
                .as_object()
                .into_iter()
                .flat_map(|sources| sources.values())
                .filter_map(Value::as_array)
                .flatten()
                .cloned()
                .collect();
            HostRow {
                id: text(&host["host_name"])
                    .or_else(|| text(&host["name"]))
                    .unwrap_or_default(),
                up: host["up"].as_bool(),
                last_reported: host["last_reported_time"].as_i64().and_then(epoch),
                apps: strings(&host["apps"]),
                cluster: tag(&Value::Array(tags), "kube_cluster_name").map(str::to_owned),
                muted: host["is_muted"].as_bool(),
            }
        })
        .collect();
    if let Some(total) = found["total_matching"].as_u64()
        && total > rows.len() as u64
    {
        ctx.note(format!("[{} of {total}; --limit N]", rows.len()));
    }
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub HOST_LIST = ["dd", "host", "list"], Read,
    "List hosts reporting to Datadog: up, last reported, apps and cluster",
    keywords: ["node", "machine", "vm", "agent reporting", "down", "infrastructure"],
    example: "dd host list --cluster prod --fields id,up,last_reported",
    run: host_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::dd;

    #[test]
    fn host_and_container_rows_name_the_cluster_and_the_pod() {
        let (outcome, transport) = dd(
            &["dd", "host", "list", "--cluster", "prod"],
            vec![Answer::json(&json!({
                "host_list": [{
                    "id": 1_588_212_437, "name": "aks-nodepool1-12345678-vmss000001", "host_name": "aks-nodepool1-12345678-vmss000001",
                    "up": true, "last_reported_time": 1_790_683_140, "apps": ["agent", "kubernetes", "docker"],
                    "is_muted": false, "tags_by_source": {"Datadog": ["kube_cluster_name:prod", "env:prod"]}
                }],
                "total_matching": 1, "total_returned": 1
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "id": "aks-nodepool1-12345678-vmss000001", "up": true, "last_reported": "2026-09-29T11:59:00Z",
                "apps": ["agent", "kubernetes", "docker"], "cluster": "prod", "muted": false
            }])
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v1/hosts?filter=kube_cluster_name%3Aprod&count=50"
        );

        let (outcome, transport) = dd(
            &["dd", "container", "list", "--deployment", "prod/web/worker"],
            vec![Answer::json(&json!({"data": [{
                "type": "container", "id": "c-1",
                "attributes": {
                    "name": "worker", "container_id": "4f5e6d7c8b9a", "state": "exited",
                    "host": "aks-nodepool1-12345678-vmss000001", "image_name": "contosoacr.azurecr.io/worker",
                    "image_tags": ["v1.4.2"], "started_at": "2026-09-29T11:47:10",
                    "tags": ["kube_cluster_name:prod", "kube_namespace:web", "pod_name:worker-5c4d3e9f1-q8zt1", "kube_deployment:worker"]
                }
            }]}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "name": "worker", "state": "exited", "image": "contosoacr.azurecr.io/worker:v1.4.2",
                "pod": "prod/web/worker-5c4d3e9f1-q8zt1", "host": "aks-nodepool1-12345678-vmss000001",
                "started": "2026-09-29T11:47:10Z"
            }])
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v2/containers?filter%5Btags%5D=kube_cluster_name%3Aprod%2Ckube_namespace%3Aweb%2Ckube_deployment%3Aworker&page%5Bsize%5D=50"
        );
    }
}
