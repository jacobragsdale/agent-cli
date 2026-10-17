//! Service connections (the REST API's service endpoints): what pipelines
//! sign in to Azure, registries and other services with. Rows name the
//! Entra app and service principal behind one, never its credential.

pub(crate) mod get;
pub(crate) mod list;

use serde_json::Value;

use crate::client::text;

/// One of `authorization.parameters`, by name in any case: Azure DevOps
/// writes `tenantid` and `serviceprincipalid` in lower case and the
/// workload identity ones in camel case. Callers name only the parameters
/// that hold no secret (never `serviceprincipalkey`, `password`, `apitoken`).
fn parameter(endpoint: &Value, name: &str) -> Option<String> {
    let parameters = endpoint["authorization"]["parameters"].as_object()?;
    parameters
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| text(value))
}
