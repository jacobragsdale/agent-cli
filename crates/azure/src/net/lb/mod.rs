//! `net lb`: load balancers. A frontend names its k8s service through the
//! public IP's tag, which `ip/mod.rs` reads.

pub(crate) mod get;
pub(crate) mod list;
