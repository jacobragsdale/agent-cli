//! The `[azure]` section every Azure domain reads.

use std::time::Duration;

use agent_cli_core::Ctx;
use anyhow::Result;
use serde::{Deserialize, Deserializer};

/// The `[azure]` section. An empty list means everything the login reaches;
/// a non-empty one is an allowlist that also fixes the order rows come in.
///
/// ```toml
/// [azure]
/// subscriptions = ["00000000-0000-0000-0000-000000000000"]  # or one string
/// vaults = ["kv-contoso-dev", "kv-contoso-prod"]
/// registries = "contosoacr"
/// search_services = ["srch-contoso-prod", "srch-contoso-dev"]
/// refresh = 300    # seconds the Resource Graph inventory, ACR attributes and index fields are cached
/// parallel = 8     # vaults, registries or repositories read at once
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Azure {
    #[serde(deserialize_with = "one_or_many")]
    pub subscriptions: Vec<String>,
    #[serde(deserialize_with = "one_or_many")]
    pub vaults: Vec<String>,
    #[serde(deserialize_with = "one_or_many")]
    pub registries: Vec<String>,
    #[serde(deserialize_with = "one_or_many")]
    pub search_services: Vec<String>,
    pub refresh: Option<u64>,
    pub parallel: Option<usize>,
}

impl Azure {
    pub fn load(ctx: &Ctx) -> Result<Self> {
        ctx.section("azure")
    }

    /// How long the inventory and registry attributes stay cached. Zero turns
    /// the cache off.
    pub fn refresh(&self) -> Duration {
        Duration::from_secs(self.refresh.unwrap_or(300))
    }

    /// Eight is well inside every plane's per-resource quota; a Basic-tier
    /// registry with hundreds of repositories is where to turn it down.
    pub fn parallel(&self) -> usize {
        self.parallel.unwrap_or(8).max(1)
    }
}

/// A key that takes one string or a list of them.
pub(crate) fn one_or_many<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(one) => vec![one],
        OneOrMany::Many(many) => many,
    })
}
