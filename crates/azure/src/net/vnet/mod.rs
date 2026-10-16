//! What `vnet list` and `vnet get` share: a peering as both print it.

pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::text;

#[derive(Debug, Serialize, JsonSchema)]
pub struct Peering {
    name: Option<String>,
    /// The other vnet's ARM id.
    remote: Option<String>,
    /// Connected, Initiated (the other side has no peering back) or Disconnected.
    state: Option<String>,
    /// FullyInSync, or LocalNotInSync / RemoteNotInSync after an address change.
    sync: Option<String>,
    forwarded_traffic: Option<bool>,
    gateway_transit: Option<bool>,
    remote_gateways: Option<bool>,
}

/// A peering from `virtualNetworkPeerings[]`.
pub(crate) fn peering(held: &Value) -> Peering {
    let props = &held["properties"];
    Peering {
        name: text(&held["name"]),
        remote: text(&props["remoteVirtualNetwork"]["id"]),
        state: text(&props["peeringState"]),
        sync: text(&props["peeringSyncLevel"]),
        forwarded_traffic: props["allowForwardedTraffic"].as_bool(),
        gateway_transit: props["allowGatewayTransit"].as_bool(),
        remote_gateways: props["useRemoteGateways"].as_bool(),
    }
}
