//! Files (Keeper's artifacts): one stored path in a repository. A path is
//! immutable unless the repository keeps versions: Keeper refuses a second
//! upload of it.

pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{quarantine, stamp, text};

/// One file, as file list and version get print it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FileRow {
    /// REPO/PATH: what file get takes.
    pub(crate) id: String,
    /// Bytes.
    pub(crate) size: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) content_type: Option<String>,
    /// When it was uploaded (upstream's upload time, for a proxy's file).
    pub(crate) created: Option<String>,
    /// Who uploaded it: a user or service account.
    pub(crate) published_by: Option<String>,
    pub(crate) downloads: Option<u64>,
    /// A proxy: whether it holds the file.
    pub(crate) cached: Option<bool>,
    /// A proxy: when it fetched the file from upstream.
    pub(crate) cached_at: Option<String>,
    /// Held back from downloads for review: quarantined, pending …
    pub(crate) quarantine: Option<String>,
}

impl FileRow {
    /// A file as `/repositories/{key}/artifacts` lists it.
    pub(crate) fn of(repo: &str, file: &Value) -> Option<Self> {
        let path = text(&file["path"])?;
        Some(Self {
            id: format!("{repo}/{path}"),
            size: file["size_bytes"].as_u64(),
            sha256: text(&file["checksum_sha256"]),
            content_type: text(&file["content_type"]),
            created: stamp(&file["created_at"]),
            published_by: text(&file["uploaded_by_username"]),
            downloads: file["download_count"].as_u64(),
            cached: None,
            cached_at: stamp(&file["cache_cached_at"]),
            quarantine: quarantine(file),
        })
    }
}
