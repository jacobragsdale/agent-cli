//! `k8s ingress diff`: two ingresses (staging against prod) by class, rules by host, paths, backends and TLS.

use agent_cli_core::{Ctx, Diff, command};
use anyhow::Result;

use crate::diff::Sides;

fn ingress_diff(ctx: &Ctx, args: Sides) -> Result<Diff> {
    let sides = args.read(ctx, "ingress")?;
    args.manifests(ctx, sides)
}

command! {
    pub INGRESS_DIFF = ["k8s", "ingress", "diff"], Read,
    "Compare two ingresses across environments: class, hosts, paths, TLS",
    keywords: ["staging", "host", "route", "tls"],
    example: "k8s ingress diff staging/web/api prod/web/api",
    run: ingress_diff,
}

#[cfg(test)]
mod tests {
    use crate::testing::k8s;

    #[test]
    fn an_ingress_against_itself_is_the_same() {
        let outcome = k8s(&["k8s", "ingress", "diff", "qa/dev/orders", "qa/dev/orders"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["same"], true);
    }
}
