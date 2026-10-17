//! `k8s job diff`: two Jobs, a failed run against a good one of the same
//! CronJob: pod template, image digests, node, start, duration and each
//! container's exit code and reason, from the newest pod of each.

use agent_cli_core::{Ctx, Diff, command, diff};
use anyhow::Result;
use serde_json::{Value, json};

use crate::diff::{RULES, Sides, side, workload};
use crate::kubectl::{Target, digest, items, owned_by};

use super::row;

fn job_diff(ctx: &Ctx, args: Sides) -> Result<Diff> {
    let [left, right] = args.read(ctx, "job")?;
    let compared = |(target, item): &(Target, Value)| -> Result<Value> {
        let name = item["metadata"]["name"].as_str().unwrap_or_default();
        let mut compared = workload(item, args.diff.all);
        if let Some(job) = row(target, item) {
            let job = serde_json::to_value(job)?;
            for field in ["status", "started", "duration"] {
                compared[field] = job[field].clone();
            }
        }
        let listed = target.json(ctx, &["get", "pods", "-o", "json"])?;
        // RFC 3339 stamps in UTC sort as text: the newest try.
        let pod = items(&listed)
            .filter(|pod| owned_by(pod, "Job", name))
            .max_by_key(|pod| {
                pod["metadata"]["creationTimestamp"]
                    .as_str()
                    .map(str::to_owned)
            });
        if let Some(pod) = pod {
            compared["node"] = pod["spec"]["nodeName"].clone();
            ran(&mut compared, pod);
        }
        Ok(compared)
    };
    let (left_row, right_row) = (compared(&left)?, compared(&right)?);
    diff(
        ctx,
        &args.diff,
        &RULES,
        side(left.0.id(&left.1), left_row),
        side(right.0.id(&right.1), right_row),
    )
}

/// Each container's digest, exit code and reason, from its pod's statuses.
fn ran(compared: &mut Value, pod: &Value) {
    for container in compared["containers"].as_array_mut().into_iter().flatten() {
        let Some(status) = pod["status"]["containerStatuses"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|status| status["name"] == container["name"])
        else {
            continue;
        };
        let ended = &status["state"]["terminated"];
        if let Some(map) = container.as_object_mut() {
            map.insert("digest".to_owned(), json!(digest(&status["imageID"])));
            map.insert("exit_code".to_owned(), ended["exitCode"].clone());
            map.insert("reason".to_owned(), ended["reason"].clone());
        }
    }
}

command! {
    pub JOB_DIFF = ["k8s", "job", "diff"], Read,
    "Compare two jobs (a failed run and a good one): image, node, duration, exit code",
    keywords: ["failed", "succeeded", "good", "bad", "run", "changed", "exit"],
    example: "k8s job diff prod/web/orders-export-29842620 prod/web/orders-export-29844060",
    run: job_diff,
}

#[cfg(test)]
mod tests {
    use crate::testing::k8s;

    #[test]
    fn two_runs_of_a_cronjob_differ_by_how_they_ran() {
        let outcome = k8s(&[
            "k8s",
            "job",
            "diff",
            "qa/qa/cleanup-29819550",
            "qa/qa/cleanup-29819580",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["as"].as_array().map(Vec::len), None, "{got}");
        assert!(
            got["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["path"] == "started"),
            "{got}"
        );
    }
}
