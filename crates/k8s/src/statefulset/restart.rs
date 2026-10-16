use agent_cli_core::{Ctx, Effect, command};
use anyhow::Result;

use crate::kubectl::{At, Changed};

#[derive(clap::Args)]
pub struct StatefulsetRestartArgs {
    /// The statefulset: its id (cluster/namespace/name), namespace/name, or name
    statefulset: String,
    #[command(flatten)]
    at: At,
}

fn statefulset_restart(ctx: &Ctx, args: StatefulsetRestartArgs) -> Result<Changed> {
    let (target, name) = args.at.named(ctx, &args.statefulset)?;
    let object = format!("statefulset/{name}");
    let said = target.write(ctx, Effect::Destructive, &["rollout", "restart", &object])?;
    Ok(Changed::new(&target, object, &said, None, None))
}

command! {
    pub STATEFULSET_RESTART = ["k8s", "statefulset", "restart"], Destructive,
    "Restart a statefulset, replacing its pods one at a time from the highest",
    keywords: ["bounce", "recycle"],
    example: "k8s statefulset restart prod/web/orders-db --yes",
    run: statefulset_restart,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{dry_run, run};

    #[test]
    fn restart_plans_a_rollout_restart_and_needs_yes() {
        let plans = dry_run(&[
            "k8s",
            "statefulset",
            "restart",
            "redis",
            "--cluster",
            "qa",
            "--namespace",
            "dev",
        ]);
        assert_eq!(
            plans[0]["run"],
            json!([
                "kubectl",
                "--context",
                "aks-qa",
                "--request-timeout=10s",
                "rollout",
                "restart",
                "statefulset/redis",
                "-n",
                "dev"
            ])
        );
        assert_eq!(run(&["k8s", "statefulset", "restart", "redis"]).code, 2);
        let outcome = run(&["k8s", "statefulset", "restart", "redis", "--yes"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["said"], "statefulset/redis restarted");
    }
}
