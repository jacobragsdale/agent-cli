use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::kubectl::{At, digest, items, limited, non_empty, owner_of};

#[derive(clap::Args)]
pub struct DeploymentListArgs {
    /// Part of the deployment name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    /// Only deployments that rolled out after this
    #[arg(long)]
    since: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeploymentRow {
    /// `cluster/namespace/name`: what `deployment restart` and `scale` take.
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

fn deployment_list(ctx: &Ctx, args: DeploymentListArgs) -> Result<Vec<DeploymentRow>> {
    let target = args.at.listing(ctx)?;
    // One call: the pods say which digest each tag resolved to.
    let listed = target.json(ctx, &["get", "deployments,pods", "-o", "json"])?;
    let (deployments, pods): (Vec<&Value>, Vec<&Value>) =
        items(&listed).partition(|item| item["kind"].as_str() == Some("Deployment"));
    let mut rows: Vec<(Option<OffsetDateTime>, DeploymentRow)> = deployments
        .into_iter()
        .filter_map(|item| {
            let name = item["metadata"]["name"].as_str()?;
            if !args.name.as_deref().is_none_or(|part| name.contains(part)) {
                return None;
            }
            let updated = rolled_out(item);
            let wanted = item["spec"]["replicas"].as_i64().unwrap_or(1);
            let namespace = item["metadata"]["namespace"].as_str();
            let own: Vec<&&Value> = pods
                .iter()
                .filter(|pod| {
                    pod["metadata"]["namespace"].as_str() == namespace
                        && owner_of(pod).as_deref() == Some(&format!("Deployment/{name}"))
                })
                .collect();
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
            let row = DeploymentRow {
                id: target.id(item),
                name: name.to_owned(),
                namespace: target.row_namespace(item),
                ready: format!(
                    "{}/{wanted}",
                    item["status"]["readyReplicas"].as_i64().unwrap_or(0)
                ),
                replicas: wanted,
                images,
                updated: updated.map(agent_cli_core::utc_time),
            };
            Some((updated, row))
        })
        .collect();
    if let Some(since) = args.since {
        rows.retain(|(updated, _)| updated.is_some_and(|at| at >= since.0));
    }
    Ok(limited(
        ctx,
        rows.into_iter().map(|(_, row)| row).collect(),
        args.limit,
    ))
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

command! {
    pub DEPLOYMENT_LIST = ["k8s", "deployment", "list"], Read,
    "List deployments: ready pods, images with tag and digest, when they rolled out",
    keywords: ["deployed", "running", "version", "image", "tag", "digest", "release", "rollout", "workloads", "prod", "production"],
    example: "k8s deployment list --cluster prod --namespace web --fields id,ready,images,updated",
    run: deployment_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{k8s, run};

    #[test]
    fn deployment_list_shows_ready_pods_images_with_digests_and_when_they_rolled_out() {
        let outcome = run(&["k8s", "deployment", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"id": "qa/dev/orders-api", "name": "orders-api", "ready": "2/2", "replicas": 2,
                "images": [{"container": "api", "image": "contosoacr.azurecr.io/team/orders-api:1.2.3",
                    "digest": "sha256:7f361af0fba5b2240abf4d78b24b30ae1ee06d320e9f0cdbabbcde359ae1a61b"}],
                "updated": "2026-09-10T08:00:00Z"})
        );
        assert_eq!(rows[1]["ready"], "0/1", "the crash-looping worker");
        assert_eq!(rows[2]["images"][1]["container"], "proxy");
        let names = |argv: &[&str]| {
            run(argv)
                .json()
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&["k8s", "deployment", "list", "billing"]),
            ["billing-api"]
        );
        assert!(names(&["k8s", "deployment", "list", "--since", "2026-09-11"]).is_empty());
        assert_eq!(
            names(&["k8s", "deployment", "list", "--since", "2026-09-09"]).len(),
            3
        );

        let everywhere = k8s(&[
            "k8s",
            "deployment",
            "list",
            "--cluster",
            "all",
            "--fields",
            "id,namespace",
        ]);
        assert_eq!(
            everywhere.json()[3],
            json!({"id": "all/qa/orders-api", "namespace": "qa"})
        );
    }
}
