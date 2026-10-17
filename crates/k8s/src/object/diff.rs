//! `k8s object diff`: two objects of any kind (a SecretProviderClass: which
//! Key Vault secrets each cluster pulls), as their manifests without the
//! bookkeeping and status.

use agent_cli_core::{Ctx, Diff, DiffArgs, Side, command, diff};
use anyhow::Result;

use crate::diff::{RULES, clean};
use crate::kind::kind_named;
use crate::kubectl::At;

#[derive(clap::Args)]
pub struct ObjectDiffArgs {
    /// The kind, as `k8s kind list` names it: secretproviderclass, certificate
    kind: String,
    /// One side: its id from object list, namespace/name, or name
    left: String,
    /// The other side, as left
    right: String,
    #[command(flatten)]
    at: At,
    #[command(flatten)]
    diff: DiffArgs,
}

fn object_diff(ctx: &Ctx, args: ObjectDiffArgs) -> Result<Diff> {
    let one = |raw: &str| -> Result<Side> {
        let at = args.at.side(raw);
        let probe = at.cluster_of(ctx, raw)?;
        let kind = kind_named(ctx, &probe, &args.kind)?;
        let (target, name) = if kind.namespaced {
            at.named(ctx, raw)?
        } else {
            at.named_cluster(ctx, raw)?
        };
        let item = target.json(ctx, &["get", &kind.qualified(), &name, "-o", "json"])?;
        Ok(Side::new(target.id(&item), clean(item, args.diff.all)))
    };
    diff(ctx, &args.diff, &RULES, one(&args.left)?, one(&args.right)?)
}

command! {
    pub OBJECT_DIFF = ["k8s", "object", "diff"], Read,
    "Compare two objects of any kind, custom resources (CRDs) too, across clusters",
    keywords: ["staging", "crd", "custom", "manifest", "secretproviderclass", "spc"],
    example: "k8s object diff secretproviderclass staging/web/worker-kv prod/web/worker-kv",
    run: object_diff,
}

#[cfg(test)]
mod tests {
    use crate::testing::k8s;

    #[test]
    fn two_custom_resources_differ_by_spec_not_bookkeeping() {
        let outcome = k8s(&[
            "k8s",
            "object",
            "diff",
            "certificate",
            "qa/dev/orders-tls",
            "qa/dev/billing-tls",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["same"], false);
        assert!(
            got["changes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|change| !change["path"].as_str().unwrap().starts_with("status")),
            "{got}"
        );
    }
}
