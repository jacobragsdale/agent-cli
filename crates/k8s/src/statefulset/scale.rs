use agent_cli_core::{Ctx, Effect, command};
use anyhow::Result;

use crate::kubectl::{At, Changed};

#[derive(clap::Args)]
pub struct StatefulsetScaleArgs {
    /// The statefulset: its id (cluster/namespace/name), namespace/name, or name
    statefulset: String,
    /// How many pods it should run; fewer removes the highest ordinals, not their claims
    #[arg(long)]
    replicas: u32,
    #[command(flatten)]
    at: At,
}

fn statefulset_scale(ctx: &Ctx, args: StatefulsetScaleArgs) -> Result<Changed> {
    let (target, name) = args.at.named(ctx, &args.statefulset)?;
    // Read first: a name that is not there fails here with exit 4, and the
    // answer says what the count was.
    let before = target.json(ctx, &["get", "statefulset", &name, "-o", "json"])?;
    let object = format!("statefulset/{name}");
    let said = target.write(
        ctx,
        Effect::Destructive,
        &["scale", &object, &format!("--replicas={}", args.replicas)],
    )?;
    Ok(Changed::new(
        &target,
        object,
        &said,
        Some(args.replicas),
        before["spec"]["replicas"].as_i64(),
    ))
}

command! {
    pub STATEFULSET_SCALE = ["k8s", "statefulset", "scale"], Destructive,
    "Scale a statefulset to a number of replicas",
    keywords: ["replicas"],
    example: "k8s statefulset scale prod/web/orders-db --replicas 0 --yes",
    run: statefulset_scale,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{dry_run, run};

    #[test]
    fn scale_reads_the_count_first_then_scales_and_says_both() {
        let plans = dry_run(&[
            "k8s",
            "statefulset",
            "scale",
            "qa/dev/redis",
            "--replicas",
            "0",
        ]);
        assert_eq!(
            plans[0]["run"].as_array().unwrap()[4..],
            json!(["scale", "statefulset/redis", "--replicas=0", "-n", "dev"])
                .as_array()
                .unwrap()[..]
        );
        let outcome = run(&[
            "k8s",
            "statefulset",
            "scale",
            "redis",
            "--replicas",
            "2",
            "--yes",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"cluster": "qa", "namespace": "dev", "object": "statefulset/redis",
                   "said": "statefulset/redis scaled", "replicas": 2, "previous": 1})
        );
        let outcome = run(&[
            "k8s",
            "statefulset",
            "scale",
            "nope",
            "--replicas",
            "2",
            "--yes",
        ]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
