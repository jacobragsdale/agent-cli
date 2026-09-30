use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, age, decoded_len, items, limited};

#[derive(clap::Args)]
pub struct SecretListArgs {
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A secret's shape. There is no field its data could go in.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SecretRow {
    /// `cluster/namespace/name`: what `secret get` takes.
    id: String,
    name: String,
    namespace: Option<String>,
    /// Opaque, kubernetes.io/tls, kubernetes.io/dockerconfigjson…
    #[serde(rename = "type")]
    kind: String,
    /// Key name to its decoded size in bytes.
    keys: BTreeMap<String, usize>,
    age: Option<String>,
}

fn secret_list(ctx: &Ctx, args: SecretListArgs) -> Result<Vec<SecretRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "secrets", "-o", "json"])?;
    let rows = items(&listed)
        .filter_map(|item| {
            Some(SecretRow {
                id: target.id(item),
                name: item["metadata"]["name"].as_str()?.to_owned(),
                namespace: target.row_namespace(item),
                kind: item["type"].as_str().unwrap_or("Opaque").to_owned(),
                keys: item["data"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(key, value)| (key.clone(), value.as_str().map_or(0, decoded_len)))
                    .collect(),
                age: age(&item["metadata"]["creationTimestamp"]),
            })
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub SECRET_LIST = ["k8s", "secret", "list"], Read,
    "List Kubernetes secrets: type, key names and sizes (never values)",
    keywords: ["password", "credential", "tls", "keys", "cluster"],
    example: "k8s secret list --cluster qa --namespace dev --fields id,type,keys",
    run: secret_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{k8s, run};

    #[test]
    fn the_json_of_a_listing_can_never_carry_a_value() {
        let outcome = run(&["k8s", "secret", "list", "--raw"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = &outcome.json()[0];
        assert_eq!(
            (&row["name"], &row["type"]),
            (&json!("orders-db"), &json!("Opaque"))
        );
        assert_eq!(
            row["keys"],
            json!({"password": 7, "username": 6}),
            "sizes, decoded"
        );
        for encoded in ["aHVudGVyMg==", "hunter2", "b3JkZXJz", "LS0tLS1CRUdJTg=="] {
            assert!(
                !outcome.stdout.contains(encoded),
                "{encoded}: {}",
                outcome.stdout
            );
        }
        let schema = serde_json::to_string(&(SECRET_LIST.returns)()).unwrap();
        assert!(
            !schema.contains("\"value\"") && !schema.contains("\"data\""),
            "{schema}"
        );

        let outcome = k8s(&["k8s", "secret", "list", "--cluster", "prod"]);
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(outcome.stderr.contains("(Forbidden)"), "{}", outcome.stderr);
    }
}
