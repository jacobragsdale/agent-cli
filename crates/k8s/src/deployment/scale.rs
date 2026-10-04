use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use serde_json::json;

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
    // A Deployment sets its ReplicaSets' counts back at once: scaling one it
    // owns would exit 0 and change nothing.
    let owner = before["metadata"]["ownerReferences"]
        .as_array()
        .and_then(|owners| {
            owners
                .iter()
                .find(|owner| owner["controller"] == true && owner["kind"] == "Deployment")
        });
    if let Some(name) = owner.and_then(|owner| owner["name"].as_str()) {
        let namespace = &before["metadata"]["namespace"];
        let id = target.id(&json!({"metadata": {"namespace": namespace, "name": name}}));
        return Err(Failure::usage(format!(
            "{object} belongs to deployment {id}, which sets its replicas back"
        ))
        .hint(format!(
            "agent-cli k8s deployment scale {id} --replicas {}",
            args.replicas
        ))
        .into());
    }
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

#[cfg(test)]
mod tests {
    use crate::testing::run;

    #[test]
    fn a_replicaset_its_deployment_owns_is_refused_naming_the_deployment() {
        let outcome = run(&[
            "k8s",
            "deployment",
            "scale",
            "replicaset/orders-api-7d9f5b",
            "--replicas",
            "5",
            "--yes",
        ]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("replicaset/orders-api-7d9f5b belongs to deployment qa/dev/orders-api"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli k8s deployment scale qa/dev/orders-api --replicas 5"),
            "{}",
            outcome.stderr
        );
    }
}
