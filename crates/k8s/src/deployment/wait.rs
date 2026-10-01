use std::time::Duration;

use agent_cli_core::{Ctx, Exit, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, finished, items, non_empty};
use crate::pod::{containers, previous_logs};

use super::{DeploymentRow, deployment_at, own_pods, rolled_out, row, workload};

/// What the wait leaves of the deadline for the read after it.
const MARGIN: Duration = Duration::from_secs(5);

#[derive(clap::Args)]
pub struct DeploymentWaitArgs {
    /// The deployment: its id (cluster/namespace/name) as `deployment list` prints it, or its name
    name: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Waited {
    #[serde(flatten)]
    row: DeploymentRow,
    /// When the rollout finished: what `dd service get --since` takes to look
    /// at the new version only. None until it has.
    rolled_out: Option<String>,
}

fn deployment_wait(ctx: &Ctx, args: DeploymentWaitArgs) -> Result<Waited> {
    let (target, raw) = deployment_at(ctx, &args.at, &args.name)?;
    let object = workload(&raw, &["deployment"], "waited for")?;
    let name = object.trim_start_matches("deployment/");
    // kubectl gives up before the deadline does, so the answer can still say
    // how far the rollout got.
    let seconds = ctx.remaining()?.saturating_sub(MARGIN).as_secs().max(1);
    let timeout = format!("--timeout={seconds}s");
    let output = ctx.read(target.kubectl(&["rollout", "status", &object, &timeout]))?;
    let gave_up = output.stderr.contains("exceeded its progress deadline");
    let late = output.stderr.contains("timed out waiting");
    if !gave_up && !late {
        finished(output)?;
    }
    let listed = target.json(ctx, &["get", "deployments,pods", "-o", "json"])?;
    let (deployments, pods): (Vec<&Value>, Vec<&Value>) =
        items(&listed).partition(|item| item["kind"].as_str() == Some("Deployment"));
    let (item, row) = deployments
        .into_iter()
        .filter(|item| item["metadata"]["name"].as_str() == Some(name))
        .find_map(|item| Some((item, row(&target, item, &pods)?)))
        .ok_or_else(|| Failure::not_found(format!("deployment {name} is gone")))?;
    let id = row.id.clone();
    if !gave_up && !late {
        let rolled = rolled_out(item).map(agent_cli_core::utc_time);
        let labels = &item["metadata"]["labels"];
        let service = non_empty(&labels["tags.datadoghq.com/service"]).unwrap_or(name);
        if let Some(since) = &rolled {
            ctx.note(format!(
                "[next: agent-cli dd service get {service} --since {since}]"
            ));
        }
        return Ok(Waited {
            row,
            rolled_out: rolled,
        });
    }
    let waited = Waited {
        row,
        rolled_out: None,
    };
    if late {
        return Err(Failure::timed_out(format!(
            "{id} is still rolling out after {seconds}s"
        ))
        .hint(format!(
            "the rollout keeps going; run the same command again: agent-cli k8s deployment wait {id}"
        ))
        .with_data(waited)
        .into());
    }
    // The pod that says why: one whose container restarted, else one not ready.
    let own = own_pods(item, &pods);
    let next = own
        .iter()
        .find_map(|pod| previous_logs(&target.id(pod), &containers(pod)))
        .or_else(|| {
            own.iter()
                .find(|pod| containers(pod).iter().any(|held| !held.ready))
                .map(|pod| format!("agent-cli k8s pod get {}", target.id(pod)))
        });
    let failure = Failure::new(
        Exit::Failed,
        format!("{id} exceeded its progress deadline: Kubernetes gave up on the rollout"),
    )
    .with_data(waited);
    Err(match next {
        Some(next) => failure.hint(next),
        None => failure,
    }
    .into())
}

command! {
    pub DEPLOYMENT_WAIT = ["k8s", "deployment", "wait"], Read,
    "Wait for a rollout: exit 0 once rolled out, 1 if it failed, 124 if still going",
    keywords: ["rollout", "deploy", "finish", "until", "verify", "progress"],
    example: "k8s deployment wait prod/web/api",
    timeout: 100,
    run: deployment_wait,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn wait_exits_0_once_rolled_out_and_names_the_service_to_check_since_then() {
        let outcome = run(&["k8s", "deployment", "wait", "orders-api"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let waited = outcome.json();
        assert_eq!(waited["id"], "qa/dev/orders-api");
        assert_eq!(waited["ready"], "2/2");
        assert_eq!(waited["rolled_out"], "2026-09-10T08:00:00Z");
        assert!(
            outcome.stderr.contains(
                "[next: agent-cli dd service get orders-api --since 2026-09-10T08:00:00Z]"
            ),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn wait_exits_1_with_the_row_when_kubernetes_gave_up_and_names_the_crashing_pods_log() {
        let outcome = run(&["k8s", "deployment", "wait", "qa/dev/orders-worker"]);
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(outcome.json()["ready"], "0/1", "the row is printed as data");
        assert!(outcome.json().get("rolled_out").is_none());
        assert!(
            outcome.stderr.contains(
                "hint: agent-cli k8s pod logs qa/dev/orders-worker-5c4d3e-q8zt --previous --tail 50"
            ),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("exceeded its progress deadline"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn wait_at_the_deadline_is_124_with_how_far_it_got() {
        let outcome = crate::testing::k8s(&[
            "k8s",
            "deployment",
            "wait",
            "qa/uat/billing-api",
            "--timeout",
            "6",
        ]);
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert_eq!(outcome.json()["ready"], "0/1");
        assert!(
            outcome
                .stderr
                .contains("hint: the rollout keeps going; run the same command again: agent-cli k8s deployment wait qa/uat/billing-api"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn wait_takes_only_a_deployment_and_a_missing_one_is_not_found() {
        let outcome = run(&["k8s", "deployment", "wait", "statefulset/redis"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        let outcome = run(&["k8s", "deployment", "wait", "nope"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert_eq!(json!(outcome.stdout.trim()), json!(""));
    }
}
