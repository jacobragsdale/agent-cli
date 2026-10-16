//! `k8s kind list`: every kind the API server serves, CRDs included, from
//! `kubectl api-resources -o wide`: what `k8s object list KIND` can read.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, limited};

use super::{KindRow, kinds};

#[derive(clap::Args)]
pub struct KindListArgs {
    /// Part of the name, kind or API group: cert, gateway, keda.sh
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn kind_list(ctx: &Ctx, args: KindListArgs) -> Result<Vec<KindRow>> {
    let target = args.at.cluster_wide(ctx)?;
    let wanted = args.name.as_deref().map(str::to_ascii_lowercase);
    let rows = kinds(ctx, &target, true)?
        .into_iter()
        .filter(|row| {
            wanted.as_deref().is_none_or(|part| {
                row.name.contains(part)
                    || row.kind.to_ascii_lowercase().contains(part)
                    || row.api_version.contains(part)
                    || row.short_names.iter().any(|short| short == part)
            })
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub KIND_LIST = ["k8s", "kind", "list"], Read,
    "List every kind a cluster has, CRDs included: names, short names, API group",
    keywords: ["crd", "crds", "custom", "resource", "definitions", "api-resources", "kubectl", "types", "installed", "operator"],
    example: "k8s kind list cert --cluster qa --fields name,kind,api_version,namespaced",
    run: kind_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn kinds_include_the_crds_a_cluster_holds_and_narrow_by_part_of_a_name() {
        let outcome = k8s(&["k8s", "kind", "list", "cert", "--cluster", "qa"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "certificates", "short_names": ["cert", "certs"],
                "api_version": "cert-manager.io/v1", "namespaced": true, "kind": "Certificate",
                "verbs": ["create", "delete", "deletecollection", "get", "list", "patch", "update", "watch"]}])
        );

        let outcome = k8s(&[
            "k8s",
            "kind",
            "list",
            "--cluster",
            "qa",
            "--limit",
            "3",
            "--fields",
            "name",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json().as_array().unwrap().len(), 3);
        assert!(outcome.stderr.contains("[3 of "), "{}", outcome.stderr);

        let outcome = k8s(&["k8s", "kind", "list", "cert", "--cluster", "prod"]);
        assert_eq!(outcome.json(), json!([]), "prod has no cert-manager");
    }
}
