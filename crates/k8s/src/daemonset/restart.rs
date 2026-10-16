use agent_cli_core::{Ctx, Effect, command};
use anyhow::Result;

use crate::kubectl::{At, Changed};

use super::named;

#[derive(clap::Args)]
pub struct DaemonsetRestartArgs {
    /// The daemonset: its id (cluster/namespace/name), namespace/name, or name
    daemonset: String,
    #[command(flatten)]
    at: At,
}

fn daemonset_restart(ctx: &Ctx, args: DaemonsetRestartArgs) -> Result<Changed> {
    let (target, name) = named(ctx, &args.at, &args.daemonset)?;
    let object = format!("daemonset/{name}");
    let said = target.write(ctx, Effect::Destructive, &["rollout", "restart", &object])?;
    Ok(Changed::new(&target, object, &said, None, None))
}

command! {
    pub DAEMONSET_RESTART = ["k8s", "daemonset", "restart"], Destructive,
    "Restart a daemonset, replacing its pod on each node in turn",
    keywords: ["bounce", "recycle", "kube-system"],
    example: "k8s daemonset restart prod/kube-system/azure-cns --yes",
    run: daemonset_restart,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{dry_run, k8s};

    #[test]
    fn restart_reaches_kube_system_whatever_the_scope_names() {
        let plans = dry_run(&["k8s", "daemonset", "restart", "prod/kube-system/kube-proxy"]);
        assert_eq!(
            plans[0]["run"],
            json!([
                "kubectl",
                "--context",
                "aks-prod",
                "--request-timeout=10s",
                "rollout",
                "restart",
                "daemonset/kube-proxy",
                "-n",
                "kube-system"
            ])
        );
        let outcome = k8s(&[
            "k8s",
            "daemonset",
            "restart",
            "qa/kube-system/cilium",
            "--yes",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"cluster": "qa", "namespace": "kube-system", "object": "daemonset/cilium",
                   "said": "daemonset/cilium restarted"})
        );
    }
}
