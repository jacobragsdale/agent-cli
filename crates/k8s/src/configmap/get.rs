use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, age};

use super::data;

#[derive(clap::Args)]
pub struct ConfigMapGetArgs {
    /// The configmap: its id (cluster/namespace/name), namespace/name, or name
    name: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ConfigMapDetail {
    id: String,
    name: String,
    namespace: String,
    age: Option<String>,
    /// Key to value; a binary key shows its size only.
    data: BTreeMap<String, String>,
}

fn configmap_get(ctx: &Ctx, args: ConfigMapGetArgs) -> Result<ConfigMapDetail> {
    let (target, name) = args.at.named(ctx, &args.name)?;
    let item = target.json(ctx, &["get", "configmap", &name, "-o", "json"])?;
    Ok(ConfigMapDetail {
        id: target.id(&item),
        name,
        namespace: target.namespace.unwrap_or_default(),
        age: age(&item["metadata"]["creationTimestamp"]),
        data: data(&item),
    })
}

command! {
    pub CONFIGMAP_GET = ["k8s", "configmap", "get"], Read,
    "Show a configmap's keys and values (binary keys show their size only)",
    keywords: ["config", "settings", "environment", "values", "variables", "hold", "contents"],
    example: "k8s configmap get qa/dev/orders-config",
    run: configmap_get,
}
