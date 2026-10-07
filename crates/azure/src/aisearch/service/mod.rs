//! `aisearch service`: what `list` and `get` share.

pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;

use crate::aisearch::auth_mode;
use crate::graph::SearchService;

/// One search service, from the inventory alone.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceRow {
    /// What --service and `aisearch service get` take.
    id: String,
    endpoint: String,
    sku: Option<String>,
    replicas: Option<u32>,
    partitions: Option<u32>,
    /// `running`, or what is wrong: provisioning, degraded, disabled, error.
    status: Option<String>,
    /// What the CLI sends: `token` (an az token) or `key` (an admin key it
    /// fetches from ARM).
    auth: &'static str,
    /// The semantic ranker's plan: free or standard; none when off.
    semantic: Option<String>,
    /// publicNetworkAccess: Enabled, Disabled or SecuredByPerimeter.
    network: Option<String>,
    subscription: String,
    resource_group: String,
    location: String,
}

fn some(text: &str) -> Option<String> {
    Some(text.to_owned()).filter(|text| !text.is_empty())
}

pub(crate) fn row(service: &SearchService) -> ServiceRow {
    ServiceRow {
        id: service.name.clone(),
        endpoint: match service.endpoint.trim_end_matches('/') {
            "" => format!("https://{}.search.windows.net", service.name),
            held => held.to_owned(),
        },
        sku: some(&service.sku),
        replicas: service.replicas,
        partitions: service.partitions,
        status: some(&service.status),
        auth: auth_mode(service),
        semantic: some(&service.semantic),
        network: some(&service.network),
        subscription: service.subscription.clone(),
        resource_group: service.resource_group.clone(),
        location: service.location.clone(),
    }
}
