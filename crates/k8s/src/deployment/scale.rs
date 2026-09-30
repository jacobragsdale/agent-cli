use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, Changed};

use super::{deployment_at, workload};

/// The kinds `kubectl scale` takes.
const SCALABLE: &[&str] = &["deployment", "statefulset", "replicaset"];

#[derive(clap::Args)]
pub struct ScaleArgs {
    /// A deployment's name or id (cluster/namespace/name), or statefulset/NAME, replicaset/NAME
    name: String,
    /// How many pods it should run
    #[arg(long)]
    replicas: u32,
    #[command(flatten)]
    at: At,
}

fn deployment_scale(ctx: &Ctx, args: ScaleArgs) -> Result<Changed> {
    let (target, name) = deployment_at(ctx, &args.at, &args.name)?;
    let object = workload(&name, SCALABLE, "scaled")?;
    // Read first: a name that is not there fails here with exit 4, and the
    // answer says what the count was.
    let before = target.json(ctx, &["get", &object, "-o", "json"])?;
    let previous = before["spec"]["replicas"].as_i64();
    let said = target.write(
        ctx,
        &["scale", &object, &format!("--replicas={}", args.replicas)],
    )?;
    Ok(Changed::new(
        &target,
        object,
        &said,
        Some(args.replicas),
        previous,
    ))
}

command! {
    pub DEPLOYMENT_SCALE = ["k8s", "deployment", "scale"], Destructive,
    "Scale a deployment to a number of replicas",
    keywords: ["replicas", "pods", "up", "down", "zero", "statefulset", "workload"],
    example: "k8s deployment scale orders-api --replicas 3 --cluster qa --namespace dev",
    run: deployment_scale,
}
