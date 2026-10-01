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

mod configmap;
mod context;
mod deployment;
mod doctor;
mod event;
mod kubectl;
mod pod;
mod secret;
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
        deployment::list::DEPLOYMENT_LIST,
        deployment::restart::DEPLOYMENT_RESTART,
        deployment::scale::DEPLOYMENT_SCALE,
        deployment::wait::DEPLOYMENT_WAIT,
        configmap::list::CONFIGMAP_LIST,
        configmap::get::CONFIGMAP_GET,
        secret::list::SECRET_LIST,
        secret::get::SECRET_GET,
        context::list::CONTEXT_LIST,
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

    #[test]
    fn read_only_mode_refuses_every_change_before_kubectl_runs() {
        agent_cli_core::testing::assert_read_only_refuses(&[K8S]);
    }
}
