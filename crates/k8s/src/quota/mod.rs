//! `k8s quota …`: what a namespace may hold (ResourceQuota) and what a
//! container that asks for nothing gets (LimitRange): the "exceeded quota"
//! a refused pod names.

pub(crate) mod list;
