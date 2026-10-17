//! `k8s networkpolicy diff`: two network policies (staging against prod) by pod selector, policy types and each ingress and egress rule.

use agent_cli_core::{Ctx, Diff, command};
use anyhow::Result;

use crate::diff::Sides;

fn networkpolicy_diff(ctx: &Ctx, args: Sides) -> Result<Diff> {
    let sides = args.read(ctx, "networkpolicy")?;
    args.manifests(ctx, sides)
}

command! {
    pub NETWORKPOLICY_DIFF = ["k8s", "networkpolicy", "diff"], Read,
    "Compare two network policies across environments: selectors and rules",
    keywords: ["staging", "egress", "ingress", "firewall", "rules"],
    example: "k8s networkpolicy diff staging/web/allow-egress prod/web/allow-egress",
    run: networkpolicy_diff,
}

#[cfg(test)]
mod tests {
    use crate::testing::k8s;

    #[test]
    fn two_network_policies_differ_by_their_rules() {
        let outcome = k8s(&[
            "k8s",
            "networkpolicy",
            "diff",
            "qa/dev/default-deny",
            "qa/dev/allow-orders-api",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["same"], false);
    }
}
