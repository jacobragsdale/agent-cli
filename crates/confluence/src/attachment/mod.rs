//! What the attachment verbs share: the row v1 answers turn into.

pub(crate) mod create;
pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{person, stamp, text};

/// What an attachment read expands.
pub(crate) const EXPAND: &str = "version,container";

#[derive(Debug, Serialize, JsonSchema)]
pub struct AttachmentRow {
    /// What attachment get takes.
    id: String,
    /// The page it is on.
    page: Option<String>,
    name: Option<String>,
    media_type: Option<String>,
    /// In bytes.
    size: Option<u64>,
    version: Option<u64>,
    /// When this version was uploaded.
    created: Option<String>,
    author: Option<String>,
    comment: Option<String>,
}

pub(crate) fn row(found: &Value) -> Option<AttachmentRow> {
    Some(AttachmentRow {
        id: text(&found["id"])?,
        page: text(&found["container"]["id"]),
        name: text(&found["title"]),
        media_type: text(&found["extensions"]["mediaType"]),
        size: found["extensions"]["fileSize"].as_u64(),
        version: found["version"]["number"].as_u64(),
        created: stamp(&found["version"]["when"]),
        author: person(&found["version"]["by"]),
        comment: text(&found["extensions"]["comment"]),
    })
}
