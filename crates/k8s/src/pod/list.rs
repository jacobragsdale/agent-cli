use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, items, limited};

use super::{PodRow, row};

#[derive(clap::Args)]
pub struct PodListArgs {
    /// Part of the pod name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    /// Only pods with this label: app=api, or a key alone (repeatable, all must hold)
    #[arg(long)]
    label: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn pod_list(ctx: &Ctx, args: PodListArgs) -> Result<Vec<PodRow>> {
    let target = args.at.listing(ctx)?;
    let selector = args.label.join(",");
    let mut argv = vec!["get", "pods", "-o", "json"];
    if !selector.is_empty() {
        argv.extend(["-l", selector.as_str()]);
    }
    let listed = target.json(ctx, &argv)?;
    let rows = items(&listed)
        .filter(|item| {
            args.name.as_deref().is_none_or(|part| {
                item["metadata"]["name"]
                    .as_str()
                    .is_some_and(|name| name.contains(part))
            })
        })
        .filter_map(|item| row(&target, item))
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub POD_LIST = ["k8s", "pod", "list"], Read,
    "List pods with status, ready, restarts, age, node and owning deployment",
    keywords: ["containers", "restarting", "crashloop", "crashing", "running", "pending", "unhealthy"],
    example: "k8s pod list --cluster qa --namespace dev --fields id,status,restarts,owner",
    run: pod_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{k8s, run};

    #[test]
    fn pod_list_reads_status_ready_restarts_and_the_deployment_behind_a_replica_set() {
        let outcome = run(&["k8s", "pod", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 5);
        assert_eq!(rows[0]["name"], "orders-api-7d9f5b-abc12");
        assert_eq!(rows[0]["status"], "Running");
        assert_eq!(rows[0]["ready"], "1/1");
        assert_eq!(rows[0]["restarts"], 2);
        assert_eq!(rows[0]["owner"], "Deployment/orders-api");
        assert_eq!(rows[0]["node"], "aks-np1-vmss000000");
        assert!(
            rows[0]["age"].as_str().unwrap().ends_with('d')
                || rows[0]["age"].as_str().unwrap().ends_with("mo")
        );
        assert!(
            rows[0].get("namespace").is_none(),
            "one namespace needs no column"
        );
        assert_eq!(rows[2]["status"], "CrashLoopBackOff");
        assert_eq!(rows[2]["ready"], "0/1");
        assert_eq!(rows[3]["ready"], "2/2");
        assert_eq!(rows[4]["owner"], "StatefulSet/redis");

        let outcome = k8s(&[
            "k8s",
            "pod",
            "list",
            "--cluster",
            "all",
            "--fields",
            "name,namespace",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows.as_array().unwrap().len(),
            10,
            "every namespace of the qa context"
        );
        assert_eq!(rows[9]["namespace"], "uat");

        let outcome = run(&["k8s", "pod", "list", "worker", "--fields", "name"]);
        assert_eq!(
            outcome.json(),
            json!([{"name": "orders-worker-5c4d3e-q8zt"}])
        );
    }

    #[test]
    fn pod_list_selects_by_label() {
        let outcome = run(&[
            "k8s",
            "pod",
            "list",
            "--label",
            "app=billing-api",
            "--fields",
            "name",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "billing-api-1a2b3c-qq111"}])
        );
        let outcome = run(&[
            "k8s",
            "pod",
            "list",
            "--label",
            "app=orders-api",
            "--label",
            "pod-template-hash",
            "--fields",
            "name",
        ]);
        assert_eq!(outcome.json().as_array().unwrap().len(), 2, "{outcome:?}");
    }
}
