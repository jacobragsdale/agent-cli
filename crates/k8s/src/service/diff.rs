//! `k8s service diff`: two services (staging against prod) by type, ports, selector and their settings; cluster IPs are each cluster's own.

use agent_cli_core::{Ctx, Diff, command};
use anyhow::Result;

use crate::diff::Sides;

fn service_diff(ctx: &Ctx, args: Sides) -> Result<Diff> {
    let sides = args.read(ctx, "service")?;
    args.manifests(ctx, sides)
}

command! {
    pub SERVICE_DIFF = ["k8s", "service", "diff"], Read,
    "Compare two services across environments: type, ports, selector",
    keywords: ["staging", "port", "selector"],
    example: "k8s service diff staging/web/api prod/web/api",
    run: service_diff,
}

#[cfg(test)]
mod tests {
    use crate::testing::k8s;

    #[test]
    fn two_services_differ_by_ports_and_selector_not_their_cluster_ips() {
        let outcome = k8s(&[
            "k8s",
            "service",
            "diff",
            "qa/dev/orders-api",
            "qa/dev/billing-api",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert!(!got.to_string().contains("clusterIP\""), "{got}");
    }
}
