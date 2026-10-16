use agent_cli_core::{Ctx, Effect, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::At;

use super::revisions;

#[derive(clap::Args)]
pub struct DeploymentUpdateArgs {
    /// The deployment: its id (cluster/namespace/name), namespace/name, or name
    deployment: String,
    /// The revision to roll back to, from deployment get's revisions
    #[arg(long)]
    revision: i64,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RolledBack {
    /// The deployment's id: what `deployment wait` takes next.
    id: String,
    /// The revision its pod template now comes from.
    revision: i64,
    /// That revision's images.
    images: Vec<String>,
    /// The revision it ran before.
    previous: Option<i64>,
    /// kubectl's own line: `deployment.apps/x rolled back`.
    said: String,
}

fn deployment_update(ctx: &Ctx, args: DeploymentUpdateArgs) -> Result<RolledBack> {
    let (target, name) = args.at.named(ctx, &args.deployment)?;
    // Read first: a name that is not there fails here with exit 4, and a
    // revision that is not there is refused naming those that are.
    let item = target.json(ctx, &["get", "deployment", &name, "-o", "json"])?;
    let id = target.id(&item);
    let listed = target.json(ctx, &["get", "replicasets", "-o", "json"])?;
    let held = revisions(&listed, &name);
    let Some(wanted) = held.iter().find(|held| held.revision == args.revision) else {
        let there: Vec<String> = held.iter().map(|held| held.revision.to_string()).collect();
        return Err(Failure::usage(format!(
            "{id} has no revision {}; it keeps {}",
            args.revision,
            if there.is_empty() {
                "none".to_owned()
            } else {
                there.join(", ")
            }
        ))
        .hint(format!(
            "agent-cli k8s deployment get {id} --fields revisions"
        ))
        .into());
    };
    let said = target.write(
        ctx,
        Effect::Destructive,
        &[
            "rollout",
            "undo",
            &format!("deployment/{name}"),
            &format!("--to-revision={}", args.revision),
        ],
    )?;
    ctx.note(format!("[next: agent-cli k8s deployment wait {id}]"));
    Ok(RolledBack {
        id,
        revision: wanted.revision,
        images: wanted.images.clone(),
        previous: item["metadata"]["annotations"]["deployment.kubernetes.io/revision"]
            .as_str()
            .and_then(|held| held.parse().ok()),
        said: said.trim().to_owned(),
    })
}

command! {
    pub DEPLOYMENT_UPDATE = ["k8s", "deployment", "update"], Destructive,
    "Roll a deployment back to an earlier version of its pod template",
    keywords: ["rollback", "undo", "revert"],
    example: "k8s deployment update prod/web/api --revision 6 --yes",
    run: deployment_update,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{dry_run, run};

    #[test]
    fn a_rollback_checks_the_revision_then_undoes_to_it_and_says_its_images() {
        let plans = dry_run(&[
            "k8s",
            "deployment",
            "update",
            "qa/dev/orders-api",
            "--revision",
            "2",
        ]);
        assert_eq!(
            plans[0]["run"].as_array().unwrap()[4..],
            json!([
                "rollout",
                "undo",
                "deployment/orders-api",
                "--to-revision=2",
                "-n",
                "dev"
            ])
            .as_array()
            .unwrap()[..]
        );

        let outcome = run(&[
            "k8s",
            "deployment",
            "update",
            "orders-api",
            "--revision",
            "9",
            "--yes",
        ]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("qa/dev/orders-api has no revision 9; it keeps 3, 2"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains(
                "hint: agent-cli k8s deployment get qa/dev/orders-api --fields revisions"
            ),
            "{}",
            outcome.stderr
        );

        let outcome = run(&[
            "k8s",
            "deployment",
            "update",
            "orders-api",
            "--revision",
            "2",
            "--yes",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rolled = outcome.json();
        assert_eq!(rolled["id"], "qa/dev/orders-api");
        assert_eq!(rolled["revision"], 2);
        assert_eq!(rolled["said"], "deployment.apps/orders-api rolled back");
        assert!(!rolled["images"].as_array().unwrap().is_empty(), "{rolled}");
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli k8s deployment wait qa/dev/orders-api]"),
            "{}",
            outcome.stderr
        );
    }
}
