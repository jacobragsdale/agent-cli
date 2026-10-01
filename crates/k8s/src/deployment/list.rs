use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use serde_json::Value;
use time::OffsetDateTime;

use crate::kubectl::{At, items, limited};

use super::{DeploymentRow, rolled_out, row};

#[derive(clap::Args)]
pub struct DeploymentListArgs {
    /// Part of the deployment name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    /// Only deployments that rolled out after this
    #[arg(long)]
    since: Option<When>,
    /// Only deployments running this image: repo:tag (api:v1.4.2), a tag alone,
    /// a full reference, or a digest (sha256:…)
    #[arg(long)]
    image: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn deployment_list(ctx: &Ctx, args: DeploymentListArgs) -> Result<Vec<DeploymentRow>> {
    let target = args.at.listing(ctx)?;
    // One call: the pods say which digest each tag resolved to.
    let listed = target.json(ctx, &["get", "deployments,pods", "-o", "json"])?;
    let (deployments, pods): (Vec<&Value>, Vec<&Value>) =
        items(&listed).partition(|item| item["kind"].as_str() == Some("Deployment"));
    let mut rows: Vec<(Option<OffsetDateTime>, DeploymentRow)> = deployments
        .into_iter()
        .filter(|item| {
            let name = item["metadata"]["name"].as_str().unwrap_or_default();
            args.name.as_deref().is_none_or(|part| name.contains(part))
        })
        .filter_map(|item| Some((rolled_out(item), row(&target, item, &pods)?)))
        .filter(|(_, row)| {
            args.image
                .as_deref()
                .is_none_or(|wanted| row.images.iter().any(|image| image.runs(wanted)))
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

        for (image, want) in [
            ("orders-api:1.2.3", &["orders-api"][..]),
            ("1.2.3", &["orders-api", "orders-worker"]),
            (
                "contosoacr.azurecr.io/team/billing-api:0.9.0",
                &["billing-api"],
            ),
            ("oss/envoy:1.28", &["billing-api"]),
            (
                "sha256:7f361af0fba5b2240abf4d78b24b30ae1ee06d320e9f0cdbabbcde359ae1a61b",
                &["orders-api"],
            ),
            (
                "contosoacr.azurecr.io/team/orders-api@sha256:7f361af0fba5b2240abf4d78b24b30ae1ee06d320e9f0cdbabbcde359ae1a61b",
                &["orders-api"],
            ),
            ("api:1.2.3", &[]),
        ] {
            assert_eq!(
                names(&["k8s", "deployment", "list", "--image", image]),
                want,
                "{image}"
            );
        }

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
