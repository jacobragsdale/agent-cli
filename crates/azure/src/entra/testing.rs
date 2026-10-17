//! What entra's tests share.

use agent_cli_core::testing::{Answer, FakeTransport, Outcome};
use serde_json::{Value, json};

pub const V1: &str = "https://graph.microsoft.com/v1.0";

pub fn entra(argv: &[&str], answers: Vec<Answer>) -> (Outcome, FakeTransport) {
    crate::testing::azure(&[crate::ENTRA], argv, answers)
}

/// One page of `rows`.
pub fn page(rows: Vec<Value>) -> Answer {
    Answer::json(&json!({ "value": rows }))
}

pub fn not_found() -> Answer {
    Answer::status(
        404,
        r#"{"error":{"code":"Request_ResourceNotFound","message":"Resource does not exist or one of its queried reference-property objects are not present."}}"#,
    )
}

pub fn urls(transport: &FakeTransport) -> Vec<String> {
    transport
        .sent()
        .iter()
        .map(|sent| sent.url.clone())
        .collect()
}
