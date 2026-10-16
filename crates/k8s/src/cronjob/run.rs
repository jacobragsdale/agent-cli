use agent_cli_core::{Ctx, Effect, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::At;

#[derive(clap::Args)]
pub struct CronjobRunArgs {
    /// The cronjob: its id (cluster/namespace/name), namespace/name, or name
    cronjob: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Started {
    /// The new job's id: what `job get` takes.
    id: String,
    /// The cronjob's id it was made from.
    cronjob: String,
    /// kubectl's own line: `job.batch/x created`.
    said: String,
}

/// A run's name: `NAME-manual-SECONDS`, cut to the 63 characters a Job's
/// name may have, since its pods' `job-name` label holds it. The seconds
/// keep two runs apart, and are too many for `owner_of` to read as a
/// scheduled run's minutes.
fn job_name(cronjob: &str) -> String {
    let suffix = format!("-manual-{}", agent_cli_core::now().unix_timestamp());
    let base = &cronjob[..cronjob.len().min(63 - suffix.len())];
    format!("{}{suffix}", base.trim_end_matches('-'))
}

fn cronjob_run(ctx: &Ctx, args: CronjobRunArgs) -> Result<Started> {
    let (target, name) = args.at.named(ctx, &args.cronjob)?;
    let job = job_name(&name);
    // The job is the cronjob's template, owned by it, so it shows in
    // `cronjob get`'s runs; the schedule and concurrency policy do not apply.
    let said = target.write(
        ctx,
        Effect::Write,
        &["create", "job", &format!("--from=cronjob/{name}"), &job],
    )?;
    let namespace = target.namespace.as_deref().unwrap_or_default();
    let id = format!("{}/{namespace}/{job}", target.scope);
    ctx.note(format!("[next: agent-cli k8s job get {id}]"));
    Ok(Started {
        id,
        cronjob: format!("{}/{namespace}/{name}", target.scope),
        said: said.trim().to_owned(),
    })
}

command! {
    pub CRONJOB_RUN = ["k8s", "cronjob", "run"], Write,
    "Run a cronjob now, as a job made from its template, off its schedule",
    keywords: ["trigger", "manual", "manually", "start", "kick", "off", "immediately", "now"],
    example: "k8s cronjob run prod/web/orders-export",
    run: cronjob_run,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::job_name;
    use crate::testing::{dry_run, k8s};

    #[test]
    fn a_run_is_a_job_made_from_the_cronjob_and_its_note_names_job_get() {
        let plans = dry_run(&["k8s", "cronjob", "run", "qa/qa/cleanup"]);
        let run = plans[0]["run"].as_array().unwrap();
        assert_eq!(
            run[4..7],
            json!(["create", "job", "--from=cronjob/cleanup"])
                .as_array()
                .unwrap()[..]
        );
        assert!(
            run[7].as_str().unwrap().starts_with("cleanup-manual-"),
            "{run:?}"
        );
        assert_eq!(run[8..], json!(["-n", "qa"]).as_array().unwrap()[..]);

        let outcome = k8s(&["k8s", "cronjob", "run", "qa/qa/cleanup"]);
        assert_eq!(outcome.code, 0, "a write needs no --yes: {outcome:?}");
        let started = outcome.json();
        let id = started["id"].as_str().unwrap();
        assert!(id.starts_with("qa/qa/cleanup-manual-"), "{started}");
        assert_eq!(started["cronjob"], "qa/qa/cleanup");
        assert!(
            outcome
                .stderr
                .contains(&format!("[next: agent-cli k8s job get {id}]")),
            "{}",
            outcome.stderr
        );

        let outcome = k8s(&["k8s", "cronjob", "run", "qa/qa/nope"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }

    #[test]
    fn a_long_cronjob_name_is_cut_so_the_job_name_fits_63_characters() {
        let name = job_name(&"a".repeat(52));
        assert_eq!(name.len(), 63, "{name}");
        assert!(name.contains("-manual-"), "{name}");
        assert!(job_name("x").starts_with("x-manual-"));
    }
}
