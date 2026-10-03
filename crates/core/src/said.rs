//! What a service says when it refuses, in its own words.

use serde_json::Value;

/// What a service says when it refuses. ARM and Key Vault write it under
/// `error.message` (with the actionable part in `error.details`), a registry
/// under `errors[0].message`, Azure DevOps under `message`, FastAPI (Airflow)
/// under `detail` (a string, or a 422's list of `{loc, msg}`), Datadog as
/// `errors: ["…"]`; anything else is worth the front of its body rather than
/// nothing. Core redacts it on the way out, like every error.
#[must_use]
pub fn failure_message(text: &str) -> String {
    let parsed = serde_json::from_str::<Value>(text).unwrap_or(Value::Null);
    if let Some(said) = listed_failures(&parsed) {
        return said;
    }
    let details: Vec<&str> = parsed["error"]["details"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|detail| detail["message"].as_str())
        .collect();
    for candidate in [
        &parsed["error"]["message"],
        &parsed["errors"][0]["message"],
        &parsed["message"],
    ] {
        if let Some(said) = candidate.as_str() {
            return if details.is_empty() {
                said.to_owned()
            } else {
                format!("{said} \u{2014} {}", details.join("; "))
            };
        }
    }
    let front: String = text.trim().chars().take(300).collect();
    if front.is_empty() {
        "(no body)".to_owned()
    } else {
        front
    }
}

/// FastAPI's `detail` and Datadog's `errors` of strings, joined; `None` when
/// the body is in neither shape.
fn listed_failures(parsed: &Value) -> Option<String> {
    if let Some(detail) = parsed["detail"].as_str() {
        return Some(detail.to_owned());
    }
    let said: Vec<String> = if let Some(details) = parsed["detail"].as_array() {
        details
            .iter()
            .filter_map(|detail| {
                let message = detail["msg"].as_str()?;
                let at: Vec<String> = detail["loc"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|part| {
                        part.as_str()
                            .map_or_else(|| part.to_string(), str::to_owned)
                    })
                    .collect();
                Some(if at.is_empty() {
                    message.to_owned()
                } else {
                    format!("{}: {message}", at.join("."))
                })
            })
            .collect()
    } else {
        // Datadog's v1 errors are strings; its v2 (JSON:API) ones objects
        // with a `detail` or `title`.
        parsed["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|error| {
                error
                    .as_str()
                    .or_else(|| error["detail"].as_str())
                    .or_else(|| error["title"].as_str())
            })
            .map(str::to_owned)
            .collect()
    };
    (!said.is_empty()).then(|| said.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_read_in_whichever_shape_the_service_wrote_it() {
        assert_eq!(failure_message(r#"{"error":{"message":"nope"}}"#), "nope");
        assert_eq!(
            failure_message(r#"{"errors":[{"message":"denied"}]}"#),
            "denied"
        );
        assert_eq!(
            failure_message(r#"{"message":"TF401232: no such item"}"#),
            "TF401232: no such item"
        );
        assert_eq!(
            failure_message(r#"{"error":{"message":"bad","details":[{"message":"why"}]}}"#),
            "bad \u{2014} why"
        );
        assert_eq!(
            failure_message(r#"{"detail":"The DAG with dag_id: `x` was not found"}"#),
            "The DAG with dag_id: `x` was not found"
        );
        assert_eq!(
            failure_message(
                r#"{"detail":[{"type":"missing","loc":["body","logical_date"],"msg":"Field required"},{"loc":["query",0],"msg":"bad"}]}"#
            ),
            "body.logical_date: Field required; query.0: bad"
        );
        assert_eq!(
            failure_message(r#"{"errors":["Forbidden","Missing scope monitors_read"]}"#),
            "Forbidden; Missing scope monitors_read"
        );
        assert_eq!(
            failure_message(
                r#"{"errors":[{"status":"404","title":"Not found","detail":"no monitor 4711"}]}"#
            ),
            "no monitor 4711"
        );
        assert_eq!(failure_message("  plain text  "), "plain text");
        assert_eq!(failure_message(""), "(no body)");
    }
}
