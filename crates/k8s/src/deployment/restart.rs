use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, Changed};

use super::{deployment_at, workload};

/// The kinds `kubectl rollout restart` takes.
const ROLLABLE: &[&str] = &["deployment", "statefulset", "daemonset"];

#[derive(clap::Args)]
pub struct RestartArgs {
    /// A deployment's name or id (cluster/namespace/name), or statefulset/NAME, daemonset/NAME
    name: String,
    #[command(flatten)]
    at: At,
}

fn deployment_restart(ctx: &Ctx, args: RestartArgs) -> Result<Changed> {
    let (target, name) = deployment_at(ctx, &args.at, &args.name)?;
    let object = workload(&name, ROLLABLE, "restarted")?;
    let said = target.write(ctx, &["rollout", "restart", &object])?;
    Ok(Changed::new(&target, object, &said, None, None))
}

command! {
    pub DEPLOYMENT_RESTART = ["k8s", "deployment", "restart"], Destructive,
    "Rollout-restart a deployment, replacing its pods one at a time",
    keywords: ["rollout", "redeploy", "bounce", "recycle", "statefulset", "daemonset", "workload"],
    example: "k8s deployment restart orders-api --cluster qa --namespace dev",
    run: deployment_restart,
}
