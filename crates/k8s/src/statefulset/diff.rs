//! `k8s statefulset diff`: two statefulsets (staging against prod) by pod template, replicas, update strategy and claim templates.

use agent_cli_core::{Ctx, Diff, command};
use anyhow::Result;

use crate::diff::Sides;

fn statefulset_diff(ctx: &Ctx, args: Sides) -> Result<Diff> {
    let sides = args.read(ctx, "statefulset")?;
    args.workloads(ctx, sides)
}

command! {
    pub STATEFULSET_DIFF = ["k8s", "statefulset", "diff"], Read,
    "Compare two statefulsets across environments: pod template, replicas, claims",
    keywords: ["staging", "image", "env"],
    example: "k8s statefulset diff staging/web/orders-db prod/web/orders-db",
    run: statefulset_diff,
}

#[cfg(test)]
mod tests {
    use crate::testing::k8s;

    #[test]
    fn a_statefulset_against_itself_is_the_same() {
        let outcome = k8s(&["k8s", "statefulset", "diff", "qa/dev/redis", "qa/dev/redis"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["same"], true);
    }
}
