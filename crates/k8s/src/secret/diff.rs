//! `k8s secret diff`: two Kubernetes secrets by type and keys, each value
//! compared in memory and printed only as `same: true|false`. A secret the
//! Secrets Store CSI driver syncs from Key Vault names the vault secret to
//! compare next.

use agent_cli_core::{Ctx, Diff, Secret, Side, command, diff};
use anyhow::Result;
use serde_json::{Value, json};

use crate::diff::{RULES, Sides};
use crate::kubectl::{Target, items, non_empty};

fn secret_diff(ctx: &Ctx, args: Sides) -> Result<Diff> {
    let [(lt, li), (rt, ri)] = args.read(ctx, "secret")?;
    let side = |target: &Target, item: &Value| {
        let mut side = Side::new(target.id(item), json!({ "type": item["type"] }));
        for (key, value) in item["data"].as_object().into_iter().flatten() {
            let value = value.as_str().unwrap_or_default();
            side.secrets
                .insert(format!("data.{key}"), Some(Secret::new(value)));
        }
        side
    };
    let answer = diff(ctx, &args.diff, &RULES, side(&lt, &li), side(&rt, &ri))?;
    let differing = answer
        .changes
        .iter()
        .find(|change| change.redacted && change.same == Some(false))
        .and_then(|change| change.path.strip_prefix("data."));
    if let Some(key) = differing
        && let (Some(left), Some(right)) = (
            synced_from(ctx, &lt, &li, key),
            synced_from(ctx, &rt, &ri, key),
        )
    {
        ctx.note(format!("[next: agent-cli kv secret diff {left} {right}]"));
    }
    Ok(answer)
}

/// The Key Vault secret (`vault/name`, as `kv secret get` takes it) a
/// SecretProviderClass in the namespace syncs into this secret's `key`.
fn synced_from(ctx: &Ctx, target: &Target, item: &Value, key: &str) -> Option<String> {
    let name = item["metadata"]["name"].as_str()?;
    let classes = target
        .json(ctx, &["get", "secretproviderclasses", "-o", "json"])
        .ok()?;
    items(&classes).find_map(|class| {
        let vault = non_empty(&class["spec"]["parameters"]["keyvaultName"])?;
        let object = class["spec"]["secretObjects"]
            .as_array()?
            .iter()
            .filter(|synced| synced["secretName"] == name)
            .flat_map(|synced| synced["data"].as_array().into_iter().flatten())
            .find(|data| data["key"] == key)
            .and_then(|data| non_empty(&data["objectName"]))?;
        Some(format!("{vault}/{object}"))
    })
}

command! {
    pub SECRET_DIFF = ["k8s", "secret", "diff"], Read,
    "Compare two Kubernetes secrets: keys, type, each value same or not (never shown)",
    keywords: ["staging", "password", "credential", "rotated", "same"],
    example: "k8s secret diff staging/web/worker-db prod/web/worker-db",
    run: secret_diff,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::k8s;

    #[test]
    fn secret_values_are_compared_but_never_printed() {
        let outcome = k8s(&[
            "k8s",
            "secret",
            "diff",
            "qa/dev/orders-db",
            "qa/dev/orders-tls",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["changes"][0],
            json!({"path": "type", "left": "Opaque", "right": "kubernetes.io/tls"})
        );
        assert_eq!(
            got["changes"][1],
            json!({"path": "data.password", "redacted": true, "left": "(set)"})
        );
        assert!(!outcome.stdout.contains("aHVudGVy"), "{}", outcome.stdout);
        let same = k8s(&[
            "k8s",
            "secret",
            "diff",
            "qa/dev/orders-db",
            "qa/dev/orders-db",
        ]);
        assert_eq!(same.json()["same"], true);
    }
}
