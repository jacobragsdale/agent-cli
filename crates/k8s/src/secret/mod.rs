//! `k8s secret`. A secret's listing carries key names and sizes only:
//! kubectl cannot leave the data out, so it is dropped here, before anything
//! is returned, and only `k8s secret get` decodes one key.

pub(crate) mod get;
pub(crate) mod list;
