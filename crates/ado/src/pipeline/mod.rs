//! Build pipelines.

pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod preview;

use agent_cli_core::Ctx;
use anyhow::Result;
use serde_json::Value;

use crate::client::{Ado, text};

/// A pipeline's definition.
fn definition(ctx: &Ctx, ado: &Ado, id: i64) -> Result<Value> {
    ado.get(ctx, &ado.code(&format!("build/definitions/{id}"), ""))
}

/// The Azure Repos repository a YAML pipeline builds, and its YAML file's
/// path there. `None` for a classic pipeline or one on GitHub.
fn yaml_home(definition: &Value) -> Option<(String, String)> {
    let repository = &definition["repository"];
    if repository["type"].as_str() != Some("TfsGit") {
        return None;
    }
    let path = text(&definition["process"]["yamlFilename"])?;
    Some((
        text(&repository["name"])?,
        path.trim_start_matches('/').to_owned(),
    ))
}
