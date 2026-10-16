//! User-assigned managed identities: Resource Graph lists them, and `get`
//! reads one with its federated credentials, each subject named as the k8s
//! serviceaccount id it trusts, and its role assignments (`role/mod.rs`).

pub(crate) mod get;
pub(crate) mod list;

pub(crate) const IDENTITY: &str = "Microsoft.ManagedIdentity/userAssignedIdentities";
