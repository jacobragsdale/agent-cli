use std::collections::BTreeMap;

use agent_cli_core::{
    Body, Ctx, Diff, DiffArgs, Method, Rules, Secret, Side, command, diff, sensitive_key,
};
use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::call::Call;

#[derive(clap::Args)]
pub struct RequestDiffArgs {
    /// The left URL (staging's)
    left: String,
    /// The right URL (prod's)
    right: String,
    #[command(flatten)]
    diff: DiffArgs,
    #[command(flatten)]
    call: Call,
}

/// What changes from call to call whatever the service holds.
const RULES: Rules = Rules {
    noise: &[
        "headers.date",
        "headers.age",
        "headers.expires",
        "headers.etag",
        "headers.last-modified",
        "headers.content-length",
        "headers.x-request-id",
        "headers.x-correlation-id",
        "headers.request-context",
        "headers.traceparent",
        "headers.tracestate",
        "headers.x-datadog-trace-id",
        "headers.x-datadog-parent-id",
        "headers.set-cookie",
        "headers.via",
        "headers.x-azure-ref",
        "headers.x-ms-request-id",
    ],
    ..Rules::PLAIN
};

fn request_diff(ctx: &Ctx, args: RequestDiffArgs) -> Result<Diff> {
    let prepared = args.call.prepare(ctx)?;
    let side = |raw: &str| -> Result<Side> {
        let url = prepared.url(raw)?;
        let response = prepared
            .send(ctx, Method::Get, &url, Body::None, false)?
            .response;
        let headers: Map<String, Value> = response
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), Value::String(value.clone())))
            .collect();
        let body = serde_json::from_str::<Value>(&response.body)
            .unwrap_or_else(|_| Value::String(response.body.clone()));
        let mut secrets = BTreeMap::new();
        let body = split_secrets(body, "body", &mut secrets);
        let mut side = Side::new(
            raw,
            json!({"status": response.status, "headers": headers, "body": body}),
        );
        side.secrets = secrets;
        Ok(side)
    };
    let left = side(&args.left)?;
    let right = side(&args.right)?;
    diff(ctx, &args.diff, &RULES, left, right)
}

/// `value` without the values that look like secrets, which go to
/// `secrets` by path so the diff says only whether they are the same.
fn split_secrets(
    value: Value,
    path: &str,
    secrets: &mut BTreeMap<String, Option<Secret>>,
) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter_map(|(key, item)| {
                    let at = format!("{path}.{key}");
                    match item {
                        Value::String(text) if sensitive_key(&key) => {
                            secrets.insert(at, Some(Secret::new(text)));
                            None
                        }
                        other => Some((key, split_secrets(other, &at, secrets))),
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| split_secrets(item, &format!("{path}.{index}"), secrets))
                .collect(),
        ),
        other => other,
    }
}

command! {
    pub REQUEST_DIFF = ["http", "request", "diff"], Read,
    "Compare two GETs (staging's URL and prod's): status, headers and the JSON body",
    keywords: ["http", "responses"],
    example: "http request diff https://api-staging.contoso.example/orders/42 https://api.contoso.example/orders/42",
    run: request_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};
    use serde_json::json;

    use crate::DOMAIN;

    #[test]
    fn two_answers_differ_in_their_bodies_not_their_noise() {
        let transport = FakeTransport::answering([
            Answer::json(&json!({"id": 42, "status": "open", "api_key": "a"}))
                .with_header("Date", "Mon")
                .with_header("Content-Type", "application/json"),
            Answer::json(&json!({"id": 42, "status": "shipped", "api_key": "b"}))
                .with_header("Date", "Tue")
                .with_header("Content-Type", "application/json"),
        ]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "request",
                "diff",
                "https://api-staging.contoso.example/orders/42",
                "https://api.contoso.example/orders/42",
            ],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let diff = outcome.json();
        let paths: Vec<&str> = diff["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["body.status", "body.api_key"]);
        assert_eq!(diff["changes"][1]["same"], false);
        assert!(!outcome.stdout.contains("\"a\"") && !outcome.stdout.contains("\"b\""));
    }
}
