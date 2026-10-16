use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, items, owned_by};
use crate::pod::PodRow;
use crate::shape::{Condition, EventRow, conditions, events_in};

use super::{JobRow, row};

#[derive(clap::Args)]
pub struct JobGetArgs {
    /// The job: its id (cluster/namespace/name), namespace/name, or name
    job: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct JobDetail {
    #[serde(flatten)]
    row: JobRow,
    /// Retries before it is marked Failed.
    backoff_limit: Option<i64>,
    /// Seconds it may run before it is stopped (DeadlineExceeded).
    deadline: Option<i64>,
    /// Complete, or Failed with BackoffLimitExceeded or DeadlineExceeded.
    conditions: Vec<Condition>,
    /// Its pods, newest first: each try's status and node.
    pods: Vec<PodRow>,
    /// The five newest events about it, newest first.
    events: Vec<EventRow>,
}

fn job_get(ctx: &Ctx, args: JobGetArgs) -> Result<JobDetail> {
    let (target, name) = args.at.named(ctx, &args.job)?;
    let item = target.json(ctx, &["get", "job", &name, "-o", "json"])?;
    let listed = target.json(ctx, &["get", "pods,events", "-o", "json"])?;
    let mut pods: Vec<&Value> = items(&listed)
        .filter(|held| held["kind"] == "Pod" && owned_by(held, "Job", &name))
        .collect();
    // RFC 3339 stamps in UTC sort as text.
    pods.sort_by_key(|pod| std::cmp::Reverse(pod["metadata"]["creationTimestamp"].as_str()));
    let row = row(&target, &item)
        .ok_or_else(|| anyhow::anyhow!("kubectl answered a job with no name"))?;
    // A failed try's log says why; its pod stays after the Job gives up.
    if row.status == "Failed"
        && let Some(pod) = pods.iter().find(|pod| pod["status"]["phase"] == "Failed")
    {
        ctx.note(format!(
            "[next: agent-cli k8s pod logs {} --tail 50]",
            target.id(pod)
        ));
    }
    Ok(JobDetail {
        row,
        backoff_limit: item["spec"]["backoffLimit"].as_i64(),
        deadline: item["spec"]["activeDeadlineSeconds"].as_i64(),
        conditions: conditions(&item),
        pods: pods
            .iter()
            .filter_map(|pod| crate::pod::row(&target, pod))
            .collect(),
        events: events_in(&target, &listed, &item),
    })
}

command! {
    pub JOB_GET = ["k8s", "job", "get"], Read,
    "Show why a job failed: its tries, conditions, the cronjob behind it, events",
    keywords: ["batch", "describe", "backofflimitexceeded", "deadlineexceeded", "retries"],
    example: "k8s job get prod/web/orders-export-29844060 --fields status,conditions,pods",
    run: job_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn a_failed_job_says_why_and_names_its_failed_pods_log() {
        let outcome = k8s(&["k8s", "job", "get", "qa/qa/cleanup-29819580"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["status"], "Failed");
        assert_eq!(got["cronjob"], "qa/qa/cleanup");
        assert_eq!(
            got["conditions"][0],
            json!({"type": "Failed", "status": "True", "reason": "BackoffLimitExceeded",
                "message": "Job has reached the specified backoff limit"})
        );
        assert_eq!(got["pods"][0]["id"], "qa/qa/cleanup-29819580-b7m4x");
        assert_eq!(got["pods"][0]["owner"], "CronJob/cleanup");
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli k8s pod logs qa/qa/cleanup-29819580-b7m4x --tail 50]"),
            "{}",
            outcome.stderr
        );

        let done = k8s(&["k8s", "job", "get", "qa/qa/nightly-report"]);
        assert_eq!(done.code, 0, "{done:?}");
        assert_eq!(done.json()["status"], "Complete");
        assert!(!done.stderr.contains("[next:"), "{}", done.stderr);
    }
}
