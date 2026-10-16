use agent_cli_core::{Ctx, Effect, command};
use anyhow::Result;

use crate::kubectl::{At, Changed};

#[derive(clap::Args)]
pub struct JobDeleteArgs {
    /// The job: its id (cluster/namespace/name), namespace/name, or name
    job: String,
    #[command(flatten)]
    at: At,
}

fn job_delete(ctx: &Ctx, args: JobDeleteArgs) -> Result<Changed> {
    let (target, job) = args.at.named(ctx, &args.job)?;
    // kubectl deletes the job's pods with it, in the background; waiting for
    // their grace periods would spend the deadline on nothing.
    let said = target.write(
        ctx,
        Effect::Destructive,
        &["delete", "job", &job, "--wait=false"],
    )?;
    Ok(Changed::new(
        &target,
        format!("job/{job}"),
        &said,
        None,
        None,
    ))
}

command! {
    pub JOB_DELETE = ["k8s", "job", "delete"], Destructive,
    "Delete a job and its pods, stopping it if it still runs",
    keywords: ["remove", "stop", "kill", "clean"],
    example: "k8s job delete prod/web/orders-export-29844060 --yes",
    run: job_delete,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{dry_run, k8s};

    #[test]
    fn delete_needs_yes_plans_under_dry_run_and_deletes_the_job_with_its_pods() {
        let argv = ["k8s", "job", "delete", "qa/qa/nightly-report"];
        let plans = dry_run(&argv);
        assert_eq!(
            plans[0]["run"],
            json!([
                "kubectl",
                "--context",
                "aks-qa",
                "--request-timeout=10s",
                "delete",
                "job",
                "nightly-report",
                "--wait=false",
                "-n",
                "qa"
            ])
        );
        assert_eq!(k8s(&argv).code, 2, "a delete needs --yes");

        let outcome = k8s(&["k8s", "job", "delete", "qa/qa/nightly-report", "--yes"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"cluster": "qa", "namespace": "qa", "object": "job/nightly-report",
                   "said": "job.batch \"nightly-report\" deleted"})
        );
    }
}
