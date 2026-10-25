//! Repositories: what `repo list` and `repo get` print of one.

pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{stamp, text};

/// What both verbs print of a repository.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RepoRow {
    /// The repository's key: what repo get takes, and the REPO in every
    /// package, version and file id.
    pub(crate) id: String,
    /// Its display name, when it is not the key.
    name: Option<String>,
    /// pypi, generic, npm, maven, docker …
    format: Option<String>,
    /// local (hosted here), remote (a proxy of upstream) or virtual (members
    /// searched in order).
    #[serde(rename = "type")]
    kind: Option<String>,
    /// public (anonymous reads), internal or private.
    visibility: Option<String>,
    /// What a remote repository proxies.
    upstream: Option<String>,
    /// Bytes stored.
    size: Option<u64>,
    created: Option<String>,
}

impl RepoRow {
    pub(crate) fn of(repo: &Value) -> Self {
        let id = text(&repo["key"]).unwrap_or_default();
        Self {
            name: text(&repo["name"]).filter(|name| *name != id),
            id,
            format: text(&repo["format"]),
            kind: text(&repo["repo_type"]),
            visibility: text(&repo["visibility"]),
            upstream: text(&repo["upstream_url"]),
            size: repo["storage_used_bytes"].as_u64(),
            created: stamp(&repo["created_at"]),
        }
    }
}
