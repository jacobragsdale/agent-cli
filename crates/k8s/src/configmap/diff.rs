//! `k8s configmap diff`: two configmaps key by key, a value holding JSON
//! compared as JSON and a file compared by lines.

use agent_cli_core::{Ctx, Diff, Side, command, diff};
use anyhow::Result;
use serde_json::json;

use crate::diff::{RULES, Sides};

use super::data;

fn configmap_diff(ctx: &Ctx, args: Sides) -> Result<Diff> {
    let [(lt, li), (rt, ri)] = args.read(ctx, "configmap")?;
    let side = |target: &crate::kubectl::Target, item| {
        Side::new(target.id(item), json!({ "data": data(item) }))
    };
    diff(ctx, &args.diff, &RULES, side(&lt, &li), side(&rt, &ri))
}

command! {
    pub CONFIGMAP_DIFF = ["k8s", "configmap", "diff"], Read,
    "Compare two configmaps across environments: keys, values, files by lines",
    keywords: ["staging", "config", "settings", "appsettings"],
    example: "k8s configmap diff staging/web/api-config prod/web/api-config",
    run: configmap_diff,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn two_configmaps_differ_by_key_with_the_environment_lined_up() {
        let outcome = k8s(&[
            "k8s",
            "configmap",
            "diff",
            "qa/dev/orders-config",
            "prod/prod/orders-config",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["as"], json!(["qa=prod", "dev=prod"]));
        // DB_HOST is orders-db.dev.svc against orders-db.prod.svc: the same.
        assert_eq!(
            got["changes"],
            json!([
                {"path": "data.FEATURES", "left": "a=1\nb=2\nc=3\n"},
                {"path": "data.LOG_LEVEL", "left": "info", "right": "warn"},
            ])
        );
        assert!(
            outcome
                .stderr
                .contains("[2 differences: data.FEATURES, data.LOG_LEVEL]"),
            "{}",
            outcome.stderr
        );
    }
}
