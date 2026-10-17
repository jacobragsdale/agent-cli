//! Groups: security, Microsoft 365, mail-enabled security and distribution,
//! assigned or dynamic.

pub(crate) mod get;
pub(crate) mod list;

use serde_json::Value;

/// What `groupTypes`, `securityEnabled` and `mailEnabled` say a group is:
/// `security`, `m365`, `mail-security` or `distribution`, and whether its
/// members come from a rule.
pub(crate) fn kind(group: &Value) -> (String, bool) {
    let types: Vec<&str> = group["groupTypes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let security = group["securityEnabled"] == true;
    let mail = group["mailEnabled"] == true;
    let kind = if types.contains(&"Unified") {
        "m365"
    } else if security && mail {
        "mail-security"
    } else if security {
        "security"
    } else {
        "distribution"
    };
    (kind.to_owned(), types.contains(&"DynamicMembership"))
}
