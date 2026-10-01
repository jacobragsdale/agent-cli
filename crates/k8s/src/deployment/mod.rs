//! `k8s deployment`: workloads, and the changes made to them.

pub(crate) mod list;
pub(crate) mod restart;
pub(crate) mod scale;
pub(crate) mod wait;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::kubectl::{At, Target, digest, non_empty, owner_of};

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeploymentRow {
    /// `cluster/namespace/name`: what `deployment restart`, `scale` and `wait` take.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// Pods ready of pods wanted: 2/3.
    ready: String,
    replicas: i64,
    images: Vec<Image>,
    /// When it last rolled out.
    updated: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Image {
    container: String,
    /// What the pod template asks for, tag included: what `acr manifest get`
    /// takes; the tag is the git tag that built it (`ado run list --branch`).
    image: String,
    /// The digest its pods run, when they report one.
    digest: Option<String>,
}

impl Image {
    /// Whether this is the image `wanted` names: a digest (alone or after
    /// `@`), the full reference, `repo:tag` without the registry, or a tag.
    fn runs(&self, wanted: &str) -> bool {
        let digest = wanted
            .rsplit_once('@')
            .map(|(_, digest)| digest)
            .or_else(|| wanted.starts_with("sha256:").then_some(wanted));
        if let Some(digest) = digest {
            return self.digest.as_deref() == Some(digest)
                || self.image.ends_with(&format!("@{digest}"));
        }
        let tag = self
            .image
            .rsplit_once(':')
            .map(|(_, tag)| tag)
            .filter(|tag| !tag.contains('/'));
        self.image == wanted || self.image.ends_with(&format!("/{wanted}")) || tag == Some(wanted)
    }
}

/// A deployment's row, with the digests its own pods (from the same read)
/// report.
fn row(target: &Target, item: &Value, pods: &[&Value]) -> Option<DeploymentRow> {
    let name = item["metadata"]["name"].as_str()?;
    let wanted = item["spec"]["replicas"].as_i64().unwrap_or(1);
    let own = own_pods(item, pods);
    let images = item["spec"]["template"]["spec"]["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|container| {
            let name = non_empty(&container["name"])?;
            let image = non_empty(&container["image"]).unwrap_or_default();
            let digest = own
                .iter()
                .flat_map(|pod| {
                    pod["status"]["containerStatuses"]
                        .as_array()
                        .into_iter()
                        .flatten()
                })
                .filter(|status| status["name"].as_str() == Some(name))
                .find_map(|status| digest(&status["imageID"]));
            Some(Image {
                container: name.to_owned(),
                image: image.to_owned(),
                digest,
            })
        })
        .collect();
    Some(DeploymentRow {
        id: target.id(item),
        name: name.to_owned(),
        namespace: target.row_namespace(item),
        ready: format!(
            "{}/{wanted}",
            item["status"]["readyReplicas"].as_i64().unwrap_or(0)
        ),
        replicas: wanted,
        images,
        updated: rolled_out(item).map(agent_cli_core::utc_time),
    })
}

/// The pods a deployment made: its ReplicaSets' pods in its namespace.
fn own_pods<'a>(item: &Value, pods: &[&'a Value]) -> Vec<&'a Value> {
    let namespace = &item["metadata"]["namespace"];
    let owner = format!(
        "Deployment/{}",
        item["metadata"]["name"].as_str().unwrap_or_default()
    );
    pods.iter()
        .copied()
        .filter(|pod| {
            &pod["metadata"]["namespace"] == namespace && owner_of(pod).as_ref() == Some(&owner)
        })
        .collect()
}

/// When a deployment last rolled out: its Progressing condition's last
/// update, which moves with each new ReplicaSet, else when it was made.
fn rolled_out(item: &Value) -> Option<OffsetDateTime> {
    let stamp = |value: &Value| OffsetDateTime::parse(value.as_str()?, &Rfc3339).ok();
    item["status"]["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|condition| condition["type"].as_str() == Some("Progressing"))
        .and_then(|condition| stamp(&condition["lastUpdateTime"]))
        .or_else(|| stamp(&item["metadata"]["creationTimestamp"]))
}

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
