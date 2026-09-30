//! `aks cluster list`.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::limited;
use crate::config::Azure;
use crate::graph::{Cluster, inventory};

#[derive(clap::Args)]
pub struct ClusterListArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A cluster, and the name the k8s domain knows it by.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ClusterRow {
    #[serde(flatten)]
    cluster: Cluster,
    /// The `[[k8s.scope]]` name `aks cluster connect` writes for it: what
    /// `k8s … --cluster` takes (k8s also accepts the context, which is the
    /// cluster's name).
    k8s_scope: String,
}

fn cluster_list(ctx: &Ctx, args: ClusterListArgs) -> Result<Vec<ClusterRow>> {
    let azure = Azure::load(ctx)?;
    let rows = inventory(ctx, &azure)?
        .clusters
        .into_iter()
        .map(|cluster| ClusterRow {
            k8s_scope: cluster.name.clone(),
            cluster,
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub CLUSTER_LIST = ["aks", "cluster", "list"], Read,
    "List the AKS clusters the az login reaches, with version and power state",
    keywords: ["kubernetes", "clusters", "managed", "running", "stopped", "version"],
    example: "aks cluster list --fields name,k8s_scope,power_state",
    run: cluster_list,
}

#[cfg(test)]
mod tests {

    use serde_json::json;

    use crate::AKS;
    use crate::testing::{self, azure};

    #[test]
    fn cluster_list_reads_the_inventory() {
        let (outcome, _) = azure(
            &[AKS],
            &["aks", "cluster", "list"],
            vec![testing::inventory(vec![
                testing::cluster("aks-contoso-dev"),
                testing::vault("kv-contoso"),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "name": "aks-contoso-dev", "resource_group": "rg-contoso",
                "subscription": "00000000-0000-0000-0000-000000000001", "location": "eastus",
                "kubernetes_version": "1.30.4", "power_state": "Running",
                "k8s_scope": "aks-contoso-dev",
            }])
        );
    }
}
