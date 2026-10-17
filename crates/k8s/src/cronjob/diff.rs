//! `k8s cronjob diff`: two cronjobs (staging against prod) by schedule, suspend, concurrency, history limits and the pod template.

use agent_cli_core::{Ctx, Diff, command};
use anyhow::Result;

use crate::diff::Sides;

fn cronjob_diff(ctx: &Ctx, args: Sides) -> Result<Diff> {
    let sides = args.read(ctx, "cronjob")?;
    args.workloads(ctx, sides)
}

command! {
    pub CRONJOB_DIFF = ["k8s", "cronjob", "diff"], Read,
    "Compare two cronjobs across environments: schedule, suspend, limits, template",
    keywords: ["staging", "schedule", "image"],
    example: "k8s cronjob diff staging/web/orders-export prod/web/orders-export",
    run: cronjob_diff,
}

#[cfg(test)]
mod tests {
    use crate::testing::k8s;

    #[test]
    fn a_cronjob_against_itself_is_the_same() {
        let outcome = k8s(&["k8s", "cronjob", "diff", "qa/qa/cleanup", "qa/qa/cleanup"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["same"], true);
    }
}
