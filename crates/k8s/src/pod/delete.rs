use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, Changed};

#[derive(clap::Args)]
pub struct PodDeleteArgs {
    /// The pod: its id (cluster/namespace/name), namespace/name, or name
    pod: String,
    #[command(flatten)]
    at: At,
}

fn pod_delete(ctx: &Ctx, args: PodDeleteArgs) -> Result<Changed> {
    let (target, pod) = args.at.named(ctx, &args.pod)?;
    // --wait=false: the pod goes Terminating and a controller replaces it;
    // waiting for the grace period would spend the deadline on nothing.
    let said = target.write(ctx, &["delete", "pod", &pod, "--wait=false"])?;
    Ok(Changed::new(
        &target,
        format!("pod/{pod}"),
        &said,
        None,
        None,
    ))
}

command! {
    pub POD_DELETE = ["k8s", "pod", "delete"], Destructive,
    "Delete a pod so its controller replaces it (a bare pod is gone for good)",
    keywords: ["kill", "restart", "evict", "remove", "recreate"],
    example: "k8s pod delete orders-api-7d9f5b-abc12 --cluster qa --namespace dev",
    run: pod_delete,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn delete_needs_yes_plans_under_dry_run_and_runs_without_waiting() {
        let outcome = run(&["k8s", "pod", "delete", "orders-api-7d9f5b-abc12"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("--yes"), "{}", outcome.stderr);

        let outcome = run(&[
            "k8s",
            "pod",
            "delete",
            "orders-api-7d9f5b-abc12",
            "--dry-run",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"dry_run": true, "would": [{"run": [
                "kubectl", "--context", "aks-qa", "--request-timeout=10s",
                "delete", "pod", "orders-api-7d9f5b-abc12", "--wait=false", "-n", "dev",
            ]}]})
        );

        let outcome = run(&["k8s", "pod", "delete", "orders-api-7d9f5b-abc12", "--yes"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"cluster": "qa", "namespace": "dev", "object": "pod/orders-api-7d9f5b-abc12",
                   "said": "pod \"orders-api-7d9f5b-abc12\" deleted"})
        );
    }
}
