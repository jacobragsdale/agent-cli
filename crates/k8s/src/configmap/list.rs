use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, age, items, limited};

use super::data;

#[derive(clap::Args)]
pub struct ConfigMapListArgs {
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ConfigMapRow {
    /// `cluster/namespace/name`: what `configmap get` takes.
    id: String,
    name: String,
    namespace: Option<String>,
    keys: Vec<String>,
    age: Option<String>,
}

fn configmap_list(ctx: &Ctx, args: ConfigMapListArgs) -> Result<Vec<ConfigMapRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "configmaps", "-o", "json"])?;
    let rows = items(&listed)
        .filter_map(|item| {
            Some(ConfigMapRow {
                id: target.id(item),
                name: item["metadata"]["name"].as_str()?.to_owned(),
                namespace: target.row_namespace(item),
                keys: data(item).into_keys().collect(),
                age: age(&item["metadata"]["creationTimestamp"]),
            })
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub CONFIGMAP_LIST = ["k8s", "configmap", "list"], Read,
    "List configmaps and their keys",
    keywords: ["config", "settings", "environment", "keys"],
    example: "k8s configmap list --cluster qa --namespace dev --fields id,keys",
    run: configmap_list,
}
