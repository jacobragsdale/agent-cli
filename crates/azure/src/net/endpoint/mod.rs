//! What `endpoint list` and `endpoint get` share: what an endpoint connects
//! to, read from its connection.

pub(crate) mod get;
pub(crate) mod list;

use serde_json::Value;

use crate::client::text;
use crate::net::{list, strings};

/// What an endpoint reaches: the target's ARM id, its group ids (`vault`,
/// `blob`) and the connection's state.
pub(crate) type Connection = (Option<String>, Vec<String>, Option<String>);

/// The first connection, auto-approved or manual: an endpoint has one.
pub(crate) fn connection(auto: &Value, manual: &Value) -> Connection {
    let Some(first) = list(auto).iter().chain(list(manual)).next() else {
        return (None, Vec::new(), None);
    };
    let props = &first["properties"];
    (
        text(&props["privateLinkServiceId"]),
        strings(&props["groupIds"]),
        text(&props["privateLinkServiceConnectionState"]["status"]),
    )
}
