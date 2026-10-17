//! `k8s daemonset diff`: two daemonsets (staging against prod) by pod template and update strategy; kube-system is read whatever the scope names.

use agent_cli_core::{Ctx, Diff, command};
use anyhow::Result;

use crate::diff::Sides;

use super::named;

fn daemonset_diff(ctx: &Ctx, args: Sides) -> Result<Diff> {
    let sides = args.read_by(ctx, "daemonset", |raw| named(ctx, &args.at, raw))?;
    args.workloads(ctx, sides)
}

command! {
    pub DAEMONSET_DIFF = ["k8s", "daemonset", "diff"], Read,
    "Compare two daemonsets across environments (CNI, agents): pod template, strategy",
    keywords: ["staging", "image", "node", "agent"],
    example: "k8s daemonset diff staging/kube-system/azure-cns prod/kube-system/azure-cns",
    run: daemonset_diff,
}

#[cfg(test)]
mod tests {
    use crate::testing::k8s;

    #[test]
    fn two_daemonsets_with_one_template_are_the_same_whatever_their_status() {
        let outcome = k8s(&[
            "k8s",
            "daemonset",
            "diff",
            "qa/kube-system/kube-proxy",
            "qa/kube-system/cilium",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["left"], "qa/kube-system/kube-proxy");
        assert_eq!(got["same"], true, "{got}");
    }
}
