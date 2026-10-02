//! Files attached to work items: the `AttachedFile` relations a work item
//! holds, which list prints and create appends to.

pub(crate) mod create;
pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{list, stamp, text};
use crate::ids::is_guid;

/// One attached file.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AttachmentRow {
    /// The attachment's GUID, which attachment get takes.
    id: String,
    name: Option<String>,
    /// In bytes.
    size: Option<u64>,
    /// When it was attached.
    date: Option<String>,
    comment: Option<String>,
}

/// A work item's attachments, from its relations (`$expand=relations`).
pub(crate) fn attachments(item: &Value) -> Vec<AttachmentRow> {
    list(&item["relations"])
        .iter()
        .filter(|relation| relation["rel"] == "AttachedFile")
        .filter_map(|relation| {
            let attributes = &relation["attributes"];
            Some(AttachmentRow {
                id: guid_in(relation["url"].as_str()?)?,
                name: text(&attributes["name"]),
                size: attributes["resourceSize"].as_u64(),
                date: stamp(&attributes["authorizedDate"]),
                comment: text(&attributes["comment"]),
            })
        })
        .collect()
}

/// The GUID ending an attachment URL's path:
/// `…/_apis/wit/attachments/{id}?fileName=x`.
pub(crate) fn guid_in(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    is_guid(last).then(|| last.to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};

    pub(crate) const SPEC: &str = "098a279a-60b9-40a8-868b-b7fd00c0a439";
    pub(crate) const LOGO: &str = "a5cedde4-2dd5-4fcf-befe-fd0977dd3433";

    /// Work item 299 with a parent link and two attachments.
    pub(crate) fn work_item() -> Value {
        json!({"id": 299, "rev": 4, "fields": {"System.Title": "Login"}, "relations": [
            {"rel": "System.LinkTypes.Hierarchy-Reverse",
             "url": "https://dev.azure.com/contoso/_apis/wit/workItems/297", "attributes": {}},
            {"rel": "AttachedFile",
             "url": format!("https://dev.azure.com/contoso/_apis/wit/attachments/{SPEC}"),
             "attributes": {"authorizedDate": "2026-09-29T20:49:26.99Z", "id": 65274,
                "resourceCreatedDate": "2026-09-29T20:49:26.99Z", "resourceSize": 12,
                "revisedDate": "9999-01-01T00:00:00Z", "comment": "Spec for the work",
                "name": "Spec.txt"}},
            {"rel": "AttachedFile",
             "url": format!("https://dev.azure.com/contoso/_apis/wit/attachments/{LOGO}"),
             "attributes": {"authorizedDate": "2026-09-30T08:00:00Z", "resourceSize": 2048,
                "name": "logo.png"}},
        ]})
    }
}
