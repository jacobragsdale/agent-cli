use agent_cli_core::{Ctx, Failure, Secret, command};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, base64_decode};

#[derive(clap::Args)]
pub struct SecretGetArgs {
    /// The secret: its id (cluster/namespace/name), namespace/name, or name
    name: String,
    /// Which key to decode
    key: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SecretValue {
    secret: String,
    key: String,
    namespace: String,
    value: String,
    /// "base64" when the bytes are not text, and `value` is left encoded.
    encoding: Option<String>,
}

fn secret_get(ctx: &Ctx, args: SecretGetArgs) -> Result<SecretValue> {
    let (target, name) = args.at.named(ctx, &args.name)?;
    let args = SecretGetArgs { name, ..args };
    let item = target.json(ctx, &["get", "secret", &args.name, "-o", "json"])?;
    let Some(encoded) = item["data"][&args.key].as_str() else {
        let keys: Vec<&str> = item["data"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, _)| key.as_str())
            .collect();
        return Err(Failure::not_found(format!(
            "secret {} has no key {:?}; its keys: {}",
            args.name,
            args.key,
            keys.join(", ")
        ))
        .into());
    };
    let bytes = base64_decode(encoded)
        .with_context(|| format!("{}/{} is not base64", args.name, args.key))?;
    let (secret, encoding) = match String::from_utf8(bytes) {
        Ok(text) => (Secret::new(text), None),
        Err(_) => (Secret::new(encoded), Some("base64".to_owned())),
    };
    Ok(SecretValue {
        secret: args.name,
        key: args.key,
        namespace: target.namespace.unwrap_or_default(),
        // The one place a Kubernetes secret's value leaves its Secret.
        value: secret.expose().to_owned(),
        encoding,
    })
}

command! {
    pub SECRET_GET = ["k8s", "secret", "get"], Reveal,
    "Decode one key of a Kubernetes secret (needs --reveal, or --output FILE)",
    keywords: ["password", "credential", "decode", "value", "cluster"],
    example: "k8s secret get orders-db password --cluster qa --namespace dev --fields value --output orders-db.txt",
    run: secret_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn secret_get_needs_reveal_decodes_one_key_and_names_the_keys_when_one_is_missing() {
        let outcome = run(&["k8s", "secret", "get", "orders-db", "password"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("--reveal"), "{}", outcome.stderr);

        let outcome = run(&["k8s", "secret", "get", "orders-db", "password", "--reveal"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"secret": "orders-db", "key": "password", "namespace": "dev", "value": "hunter2"})
        );

        let outcome = run(&["k8s", "secret", "get", "orders-db", "token", "--reveal"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("its keys: password, username"),
            "{}",
            outcome.stderr
        );
        let outcome = run(&["k8s", "secret", "get", "nope", "password", "--reveal"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
