//! `k8s object`: any kind the API server serves, CRDs included, for what no
//! curated command covers. It still bounds the answer (`--limit`, trimmed
//! manifests), prints ids `k8s object get` takes and keeps to the scopes.

pub(crate) mod diff;
pub(crate) mod get;
pub(crate) mod list;
