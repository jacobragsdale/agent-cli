//! The k8s domain: a thin, bounded, JSON-emitting `kubectl` wrapper, not a
//! kubectl replacement. Ported from az-tui's `kube.rs` (the types, the
//! `Kubectl` source, owner resolution and `kubectl_error`; not the watcher).
//!
//! Every call is `kubectl --context C --request-timeout=10s …`, run through
//! `ctx.read` or `ctx.write` and so bounded by the command's deadline, with
//! the whole process group killed at it: a kubelogin waiting on a device-code
//! prompt cannot hang the command. Where a call goes comes from
//! `[[k8s.scope]]`: `--cluster` picks a scope and `--namespace` one of its
//! namespaces, and each defaults only when there is exactly one to pick.

mod access;
mod configmap;
mod context;
mod cronjob;
mod daemonset;
mod deployment;
mod doctor;
mod event;
mod hpa;
mod ingress;
mod job;
mod kind;
mod kubectl;
mod namespace;
mod networkpolicy;
mod node;
mod object;
mod pdb;
mod pod;
mod pv;
mod pvc;
mod quota;
mod secret;
mod service;
mod serviceaccount;
mod shape;
mod statefulset;
mod storageclass;
#[cfg(test)]
mod testing;

use agent_cli_core::Domain;

pub const K8S: Domain = Domain {
    name: "k8s",
    summary: "Kubernetes",
    commands: &[
        pod::list::POD_LIST,
        pod::get::POD_GET,
        pod::logs::POD_LOGS,
        pod::delete::POD_DELETE,
        event::list::EVENT_LIST,
        service::list::SERVICE_LIST,
        service::get::SERVICE_GET,
        ingress::list::INGRESS_LIST,
        ingress::get::INGRESS_GET,
        networkpolicy::list::NETWORKPOLICY_LIST,
        networkpolicy::get::NETWORKPOLICY_GET,
        pvc::list::PVC_LIST,
        pvc::get::PVC_GET,
        pv::list::PV_LIST,
        pv::get::PV_GET,
        storageclass::list::STORAGECLASS_LIST,
        deployment::list::DEPLOYMENT_LIST,
        deployment::get::DEPLOYMENT_GET,
        deployment::restart::DEPLOYMENT_RESTART,
        deployment::scale::DEPLOYMENT_SCALE,
        deployment::wait::DEPLOYMENT_WAIT,
        statefulset::list::STATEFULSET_LIST,
        statefulset::get::STATEFULSET_GET,
        daemonset::list::DAEMONSET_LIST,
        daemonset::get::DAEMONSET_GET,
        job::list::JOB_LIST,
        job::get::JOB_GET,
        cronjob::list::CRONJOB_LIST,
        cronjob::get::CRONJOB_GET,
        hpa::list::HPA_LIST,
        pdb::list::PDB_LIST,
        node::list::NODE_LIST,
        node::get::NODE_GET,
        namespace::list::NAMESPACE_LIST,
        quota::list::QUOTA_LIST,
        serviceaccount::list::SERVICEACCOUNT_LIST,
        serviceaccount::get::SERVICEACCOUNT_GET,
        access::list::ACCESS_LIST,
        configmap::list::CONFIGMAP_LIST,
        configmap::get::CONFIGMAP_GET,
        secret::list::SECRET_LIST,
        secret::get::SECRET_GET,
        context::list::CONTEXT_LIST,
        context::get::CONTEXT_GET,
        kind::list::KIND_LIST,
        object::list::OBJECT_LIST,
        object::get::OBJECT_GET,
    ],
    synonyms: &[
        ("container", &["pod"]),
        ("containers", &["pod"]),
        ("kubernetes", &["k8s"]),
        ("kubectl", &["k8s"]),
        // Not "restarts": its stem is the restart verb, and a crash loop is
        // a question for pod list, not a reason to restart anything.
        ("crashloop", &["pod", "status", "ready"]),
        ("crashlooping", &["pod", "status", "ready"]),
        ("crash loop", &["pod", "status", "ready"]),
        ("crash looping", &["pod", "status", "ready"]),
        ("crashing", &["pod", "status", "ready"]),
        ("crash", &["pod", "previous"]),
        ("oomkilled", &["pod", "status"]),
        ("rollout", &["deployment"]),
        ("deployed", &["deployment", "images"]),
        ("redeploy", &["deployment", "restart"]),
        ("bounce", &["restart"]),
        ("replicas", &["scale"]),
        ("config map", &["configmap"]),
        ("env vars", &["configmap"]),
        ("k8s secret", &["k8s", "secret"]),
        ("kubernetes secret", &["k8s", "secret"]),
        ("log", &["logs"]),
        ("output", &["logs"]),
        ("warnings", &["event"]),
    ],
    status: doctor::status,
    doctor: doctor::doctor,
};

#[cfg(test)]
mod tests {
    use agent_cli_core::{check_layout, check_registry};

    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[K8S]), Vec::<String>::new());
        assert_eq!(check_layout(&[K8S]), Vec::<String>::new());
    }
}
