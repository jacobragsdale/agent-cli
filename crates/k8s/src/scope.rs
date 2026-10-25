//! `[[k8s.scope]]`: the clusters and namespaces commands may reach, and
//! whether each is read-only.

use agent_cli_core::{Config, Failure};
use anyhow::Result;
use serde::{Deserialize, Deserializer};

/// The `[k8s]` section.
///
/// ```toml
/// [[k8s.scope]]
/// name = "dev"                   # what --cluster takes
/// context = "aks-contoso-dev"    # the kubeconfig context; the name when left out
/// namespaces = ["web", "jobs"]   # or one string; left out: every namespace
/// read_only = true               # refuse every change here
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct K8s {
    scope: Vec<Scope>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Scope {
    pub(crate) name: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub(crate) namespaces: Vec<String>,
    /// Refuses every change in this scope; overrides `[k8s] read_only`.
    #[serde(default)]
    pub(crate) read_only: Option<bool>,
}

impl Scope {
    pub(crate) fn context(&self) -> &str {
        self.context.as_deref().unwrap_or(&self.name)
    }
}

fn one_or_many<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
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

pub(crate) fn scopes(config: &Config) -> Result<Vec<Scope>> {
    let section: K8s = config.section("k8s")?;
    if let Some(blank) = section
        .scope
        .iter()
        .find(|scope| scope.name.trim().is_empty())
    {
        return Err(Failure::setup(format!(
            "a [[k8s.scope]] in {} has a blank name (context {:?})",
            config.path().display(),
            blank.context()
        ))
        .hint("give every [[k8s.scope]] a name")
        .into());
    }
    Ok(section.scope)
}
