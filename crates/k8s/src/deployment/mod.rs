//! `k8s deployment`: workloads, and the changes made to them.

pub(crate) mod list;
pub(crate) mod restart;
pub(crate) mod scale;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;

use crate::kubectl::{At, Target};

/// A deployment's id (`cluster/namespace/name`) picks the scope and
/// namespace; `KIND/NAME` or a bare name keeps the flags'.
fn deployment_at(ctx: &Ctx, at: &At, raw: &str) -> Result<(Target, String)> {
    if raw.matches('/').count() == 2 {
        return at.named(ctx, raw);
    }
    Ok((at.one(ctx)?, raw.to_owned()))
}

/// `NAME` is a deployment; `KIND/NAME` names another workload, refused
/// unless `allowed` has it, as az-tui refused them.
fn workload(raw: &str, allowed: &[&str], done: &str) -> Result<String> {
    let (kind, name) = raw.split_once('/').unwrap_or(("deployment", raw));
    let kind = kind.to_ascii_lowercase();
    if name.is_empty() {
        return Err(Failure::usage(format!("{raw:?} names no workload")).into());
    }
    if !allowed.contains(&kind.as_str()) {
        return Err(Failure::usage(format!(
            "a {kind} cannot be {done}; only a {} can",
            allowed.join(", a ")
        ))
        .into());
    }
    Ok(format!("{kind}/{name}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn restart_and_scale_refuse_kinds_they_cannot_act_on() {
        for (argv, want) in [
            (
                &["k8s", "deployment", "restart", "replicaset/x", "--yes"][..],
                "a replicaset cannot be restarted",
            ),
            (
                &[
                    "k8s",
                    "deployment",
                    "scale",
                    "daemonset/x",
                    "--replicas",
                    "2",
                    "--yes",
                ][..],
                "a daemonset cannot be scaled",
            ),
            (
                &[
                    "k8s",
                    "deployment",
                    "restart",
                    "job/nightly-report",
                    "--yes",
                ][..],
                "a job cannot be restarted",
            ),
        ] {
            let outcome = run(argv);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(outcome.stderr.contains(want), "{}", outcome.stderr);
        }
    }

    #[test]
    fn restart_and_scale_plan_under_dry_run_and_scale_reads_the_count_first() {
        let outcome = run(&["k8s", "deployment", "restart", "orders-api", "--dry-run"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["would"][0]["run"],
            json!([
                "kubectl",
                "--context",
                "aks-qa",
                "--request-timeout=10s",
                "rollout",
                "restart",
                "deployment/orders-api",
                "-n",
                "dev"
            ])
        );
        let outcome = run(&[
            "k8s",
            "deployment",
            "scale",
            "statefulset/redis",
            "--replicas",
            "0",
            "--dry-run",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["would"][0]["run"].as_array().unwrap()[4..],
            json!(["scale", "statefulset/redis", "--replicas=0", "-n", "dev"])
                .as_array()
                .unwrap()[..]
        );

        let outcome = run(&[
            "k8s",
            "deployment",
            "scale",
            "orders-api",
            "--replicas",
            "4",
            "--yes",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"cluster": "qa", "namespace": "dev", "object": "deployment/orders-api",
                   "said": "deployment/orders-api scaled", "replicas": 4, "previous": 2})
        );
        let outcome = run(&["k8s", "deployment", "restart", "orders-api", "--yes"]);
        assert_eq!(outcome.json()["said"], "deployment/orders-api restarted");
    }
}
