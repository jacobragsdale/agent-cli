//! What the attachment verbs share: the row v2 answers turn into.

pub(crate) mod create;
pub(crate) mod get;
pub(crate) mod list;

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{stamp, text};

#[derive(Debug, Serialize, JsonSchema)]
pub struct AttachmentRow {
    /// What attachment get takes: att7001.
    id: String,
    /// The page it is on.
    page: Option<String>,
    name: Option<String>,
    media_type: Option<String>,
    /// In bytes.
    size: Option<u64>,
    version: Option<u64>,
    created: Option<String>,
    author: Option<String>,
    comment: Option<String>,
}

pub(crate) fn row(found: &Value, names: &HashMap<String, String>) -> Option<AttachmentRow> {
    Some(AttachmentRow {
        id: text(&found["id"])?,
        page: text(&found["pageId"]).or_else(|| text(&found["blogPostId"])),
        name: text(&found["title"]),
        media_type: text(&found["mediaType"]),
        size: found["fileSize"].as_u64(),
        version: found["version"]["number"].as_u64(),
        created: stamp(&found["createdAt"]),
        author: text(&found["version"]["authorId"]).map(|id| names.get(&id).cloned().unwrap_or(id)),
        comment: text(&found["comment"]),
    })
}
