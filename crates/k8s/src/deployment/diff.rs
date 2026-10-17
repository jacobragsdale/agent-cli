//! `k8s deployment diff`: two deployments (staging against prod), or one
//! against an earlier revision of itself, by pod template, images and
//! digests, replicas and strategy.

use agent_cli_core::{Ctx, Diff, DiffArgs, Failure, Rules, Side, command, diff};
use anyhow::Result;
use serde_json::{Map, Value};

use crate::diff::{RULES, next_secret, side, template, workload};
use crate::kubectl::{At, Target, items, owned_by};

use super::row;

#[derive(clap::Args)]
pub struct DeploymentDiffArgs {
    /// One side: its id (cluster/namespace/name), namespace/name, or name
    left: String,
    /// The other side; left out, the deployment against its previous revision
    right: Option<String>,
    /// With one side, the revision to compare against (deployment get lists them)
    #[arg(long)]
    revision: Option<i64>,
    #[command(flatten)]
    at: At,
    #[command(flatten)]
    diff: DiffArgs,
}

fn deployment_diff(ctx: &Ctx, args: DeploymentDiffArgs) -> Result<Diff> {
    let Some(right) = &args.right else {
        return against_revision(ctx, &args);
    };
    if args.revision.is_some() {
        return Err(Failure::usage(
            "--revision compares one deployment with its own past; give one side",
        )
        .into());
    }
    let (left_target, left_id, left_row) = read(ctx, &args.at, &args.left, args.diff.all)?;
    let (right_target, right_id, right_row) = read(ctx, &args.at, right, args.diff.all)?;
    let next = next_secret((&left_target, &left_row), (&right_target, &right_row));
    let answer = diff(
        ctx,
        &args.diff,
        &RULES,
        side(left_id, left_row),
        side(right_id, right_row),
    )?;
    if let Some(next) = next.filter(|_| answer.same) {
        ctx.note(next);
    }
    Ok(answer)
}

/// A deployment's row, each container with the digest its pods run, so a
/// tag pushed again over a new image still differs.
fn read(ctx: &Ctx, at: &At, raw: &str, all: bool) -> Result<(Target, String, Value)> {
    let (target, name) = at.side(raw).named(ctx, raw)?;
    let item = target.json(ctx, &["get", "deployment", &name, "-o", "json"])?;
    let listed = target.json(ctx, &["get", "pods", "-o", "json"])?;
    let pods: Vec<&Value> = items(&listed).collect();
    let mut compared = workload(&item, all);
    let images = row(&target, &item, &pods)
        .map(|row| row.images)
        .unwrap_or_default();
    for container in compared["containers"].as_array_mut().into_iter().flatten() {
        let digest = images
            .iter()
            .find(|image| container["name"] == image.container.as_str())
            .and_then(|image| image.digest.clone());
        if let (Some(digest), Some(map)) = (digest, container.as_object_mut()) {
            map.insert("digest".to_owned(), Value::String(digest));
        }
    }
    let id = target.id(&item);
    Ok((target, id, compared))
}

/// The current pod template against a revision's (its ReplicaSet's): the
/// previous one unless `--revision` names another.
fn against_revision(ctx: &Ctx, args: &DeploymentDiffArgs) -> Result<Diff> {
    let (target, name) = args.at.named(ctx, &args.left)?;
    let item = target.json(ctx, &["get", "deployment", &name, "-o", "json"])?;
    let id = target.id(&item);
    let listed = target.json(ctx, &["get", "replicasets", "-o", "json"])?;
    let revision_of = |held: &Value| -> Option<i64> {
        held["metadata"]["annotations"]["deployment.kubernetes.io/revision"]
            .as_str()?
            .parse()
            .ok()
    };
    let current = revision_of(&item).unwrap_or_default();
    let mut sets: Vec<(i64, &Value)> = items(&listed)
        .filter(|held| owned_by(held, "Deployment", &name))
        .filter_map(|set| Some((revision_of(set)?, set)))
        .collect();
    sets.sort_by_key(|(revision, _)| std::cmp::Reverse(*revision));
    let wanted = match args.revision {
        Some(wanted) => sets.iter().find(|(revision, _)| *revision == wanted),
        None => sets.iter().find(|(revision, _)| *revision < current),
    };
    let Some((revision, set)) = wanted else {
        let kept: Vec<String> = sets
            .iter()
            .map(|(revision, _)| revision.to_string())
            .collect();
        let what = args.revision.map_or_else(
            || "no revision before this one".to_owned(),
            |n| format!("no revision {n}"),
        );
        return Err(Failure::not_found(format!(
            "{id} keeps {what}; it keeps {}",
            if kept.is_empty() {
                "none".to_owned()
            } else {
                kept.join(", ")
            }
        ))
        .hint(format!(
            "agent-cli k8s deployment get {id} --fields revisions"
        ))
        .into());
    };
    let rows = |spec: &Value| {
        Value::Object(
            template(spec, args.diff.all)
                .into_iter()
                .collect::<Map<_, _>>(),
        )
    };
    let rules = Rules {
        infer: false,
        ..RULES
    };
    diff(
        ctx,
        &args.diff,
        &rules,
        Side::new(format!("{id}@{revision}"), rows(&set["spec"]["template"])),
        Side::new(format!("{id}@{current}"), rows(&item["spec"]["template"])),
    )
}

command! {
    pub DEPLOYMENT_DIFF = ["k8s", "deployment", "diff"], Read,
    "Compare two deployments across environments, or one with its previous revision",
    keywords: ["staging", "changed", "last", "rollout", "env", "image"],
    example: "k8s deployment diff staging/web/api prod/web/api",
    run: deployment_diff,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn two_deployments_differ_by_image_and_replicas_with_the_environment_lined_up() {
        let outcome = k8s(&[
            "k8s",
            "deployment",
            "diff",
            "qa/dev/orders-api",
            "prod/prod/orders-api",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["left"], "qa/dev/orders-api");
        assert_eq!(got["as"], json!(["qa=prod", "dev=prod"]));
        assert!(
            outcome
                .stderr
                .contains("[treating qa as prod, dev as prod;"),
            "{}",
            outcome.stderr
        );
        assert!(got["changes"].as_array().is_some(), "{got}");
    }

    #[test]
    fn one_deployment_compares_with_its_previous_revision() {
        let outcome = k8s(&["k8s", "deployment", "diff", "qa/dev/orders-api"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["left"], "qa/dev/orders-api@2");
        assert_eq!(got["right"], "qa/dev/orders-api@3");
        assert_eq!(
            got["changes"][0],
            json!({"path": "containers.api.image",
                "left": "contosoacr.azurecr.io/team/orders-api:1.2.2",
                "right": "contosoacr.azurecr.io/team/orders-api:1.2.3"})
        );
        let outcome = k8s(&[
            "k8s",
            "deployment",
            "diff",
            "qa/dev/orders-api",
            "--revision",
            "9",
        ]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("it keeps 3, 2"),
            "{}",
            outcome.stderr
        );
        let outcome = k8s(&[
            "k8s",
            "deployment",
            "diff",
            "qa/dev/a",
            "qa/dev/b",
            "--revision",
            "2",
        ]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
