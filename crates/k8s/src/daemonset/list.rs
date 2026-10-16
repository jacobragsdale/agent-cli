use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, items, limited, name_has};

use super::{DaemonSetRow, listing, row};

#[derive(clap::Args)]
pub struct DaemonsetListArgs {
    /// Part of the daemonset name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn daemonset_list(ctx: &Ctx, args: DaemonsetListArgs) -> Result<Vec<DaemonSetRow>> {
    let target = listing(ctx, &args.at)?;
    let listed = target.json(ctx, &["get", "daemonsets", "-o", "json"])?;
    let rows = items(&listed)
        .filter(|item| name_has(item, args.name.as_deref()))
        .filter_map(|item| row(&target, item))
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub DAEMONSET_LIST = ["k8s", "daemonset", "list"], Read,
    "List daemonsets, kube-system's too: desired, ready, unavailable per node",
    keywords: ["ds", "every", "node", "agent", "cni", "csi", "kube-proxy", "unavailable"],
    example: "k8s daemonset list --cluster prod --namespace kube-system --fields id,desired,ready,unavailable",
    run: daemonset_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn kube_system_daemonsets_list_whatever_namespaces_the_scope_names() {
        let outcome = k8s(&[
            "k8s",
            "daemonset",
            "list",
            "--cluster",
            "qa",
            "--namespace",
            "kube-system",
            "--fields",
            "id,desired,ready,unavailable",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "qa/kube-system/kube-proxy", "desired": 2, "ready": 2, "unavailable": 0},
                {"id": "qa/kube-system/cilium", "desired": 2, "ready": 1, "unavailable": 1}])
        );
        let outcome = k8s(&[
            "k8s",
            "daemonset",
            "list",
            "--cluster",
            "qa",
            "--namespace",
            "other",
        ]);
        assert_eq!(
            outcome.code, 2,
            "other namespaces keep to the scope: {outcome:?}"
        );
    }
}
