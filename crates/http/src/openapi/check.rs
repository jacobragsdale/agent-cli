//! A call held to its operation before it goes (`request run --spec`), and
//! the answer held to what the operation documents.
// ponytail: checks type, required, enum, nullable and additionalProperties
// false only; no if/then, patternProperties, $dynamicRef or formats. A JSON
// Schema crate if specs that lean on those turn up.

use agent_cli_core::{Body, Failure};
use anyhow::Result;
use serde_json::Value;

use super::fields::{Node, texts};
use super::{Operation, Spec};
use crate::url;

/// The most problems one check names.
const MOST: usize = 20;

/// The operation a call is, and how it differs from it.
pub(crate) fn request(
    spec: &Spec,
    method: &str,
    address: &str,
    body: &Body,
) -> Result<(Operation, Vec<String>)> {
    let path = url::path(address);
    let base_path = spec.base().map(|base| url::path(&base)).unwrap_or_default();
    let path = path
        .strip_prefix(base_path.trim_end_matches('/'))
        .filter(|rest| rest.starts_with('/'))
        .unwrap_or(&path)
        .to_owned();
    let mut candidates: Vec<&Operation> = spec
        .operations
        .iter()
        .filter(|op| op.method == method && matches(&op.path, &path))
        .collect();
    // The literal path beats a template: /orders/search before /orders/{id}.
    candidates.sort_by_key(|op| op.path.matches('{').count());
    let Some(operation) = candidates.first() else {
        return Err(
            Failure::usage(format!("{} has no operation {method} {path}", spec.id))
                .hint(format!(
                    "agent-cli http operation list {} --path '{path}'  shows what it serves there",
                    spec.id
                ))
                .into(),
        );
    };
    let mut problems = Vec::new();
    let query: Vec<String> = address
        .split_once('?')
        .map(|(_, query)| {
            query
                .split('&')
                .filter_map(|pair| pair.split('=').next())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    for param in &operation.parameters {
        if param.location == "query" && param.required && !query.contains(&param.name) {
            problems.push(format!("the query parameter {} is required", param.name));
        }
    }
    match (body, &operation.body) {
        (Body::None, Some(wanted)) if wanted.required => {
            problems.push("a body is required".to_owned())
        }
        (Body::Json(document), Some(_)) => {
            if let Some(schema) = operation.body_schema() {
                check(spec, schema, document, "body", &mut problems, 0);
            }
        }
        (Body::Json(_) | Body::Form(_) | Body::Bytes(_), None)
            if matches!(method, "GET" | "DELETE" | "HEAD") => {}
        _ => {}
    }
    problems.truncate(MOST);
    Ok(((*operation).clone(), problems))
}

/// How an answer differs from what its operation documents.
pub(crate) fn response(spec: &Spec, operation: &Operation, status: u16, raw: &str) -> Vec<String> {
    let code = status.to_string();
    let range = format!("{}XX", code.chars().next().unwrap_or('0'));
    let documented = operation
        .responses
        .iter()
        .find(|doc| doc.status == code)
        .or_else(|| {
            operation
                .responses
                .iter()
                .find(|doc| doc.status.eq_ignore_ascii_case(&range))
        })
        .or_else(|| {
            operation
                .responses
                .iter()
                .find(|doc| doc.status == "default")
        });
    let Some(documented) = documented else {
        let known: Vec<&str> = operation
            .responses
            .iter()
            .map(|doc| doc.status.as_str())
            .collect();
        return vec![format!(
            "{status} is not a status it documents ({})",
            known.join(", ")
        )];
    };
    let schema = documented
        .content
        .iter()
        .find(|(media, _)| media.contains("json"))
        .map(|(_, schema)| schema);
    let (Some(schema), Ok(document)) = (schema, serde_json::from_str::<Value>(raw)) else {
        return Vec::new();
    };
    let mut problems = Vec::new();
    check(spec, schema, &document, "response", &mut problems, 0);
    problems.truncate(MOST);
    problems
}

/// True when a path the spec writes (`/orders/{id}`) matches one sent.
fn matches(template: &str, path: &str) -> bool {
    let wanted: Vec<&str> = template.trim_end_matches('/').split('/').collect();
    let given: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    wanted.len() == given.len()
        && wanted.iter().zip(&given).all(|(want, held)| {
            (want.starts_with('{') && want.ends_with('}') && !held.is_empty()) || want == held
        })
}

fn check(
    spec: &Spec,
    schema: &Value,
    value: &Value,
    path: &str,
    problems: &mut Vec<String>,
    depth: usize,
) {
    if depth > 12 || problems.len() >= MOST {
        return;
    }
    let node = Node::of(spec, schema);
    if value.is_null() {
        if !node.nullable && node.kind != "any" {
            problems.push(format!("{path} is null, which it may not be"));
        }
        return;
    }
    if !node.variants.is_empty() {
        let fits = node.variants.iter().any(|variant| {
            let mut own = Vec::new();
            check(spec, variant, value, path, &mut own, depth + 1);
            own.is_empty()
        });
        if !fits {
            problems.push(format!("{path} fits none of its oneOf/anyOf schemas"));
        }
        return;
    }
    let held = kind_of(value);
    let fits = node.kind.split('|').any(|kind| match kind {
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "object" | "map" => value.is_object(),
        "" | "any" => true,
        kind => kind == held,
    });
    if !fits {
        problems.push(format!("{path} is {held}, not {}", node.kind));
        return;
    }
    if !node.values.is_empty() && !node.values.contains(value) {
        problems.push(format!(
            "{path} is {value}, not one of {}",
            texts(&node.values).join(", ")
        ));
    }
    match value {
        Value::Object(object) => {
            for key in &node.required {
                if !object.contains_key(key) {
                    problems.push(format!("{path}.{key} is required"));
                }
            }
            for (key, item) in object {
                let at = format!("{path}.{key}");
                match node.properties.iter().find(|(name, _)| name == key) {
                    Some((_, sub)) => check(spec, sub, item, &at, problems, depth + 1),
                    None => match &node.additional {
                        Some(sub @ Value::Object(_)) => {
                            check(spec, sub, item, &at, problems, depth + 1)
                        }
                        _ if node.closed => problems.push(format!("{at} is not a field it takes")),
                        _ => {}
                    },
                }
            }
        }
        Value::Array(items) => {
            if let Some(sub) = &node.items {
                for (index, item) in items.iter().enumerate().take(50) {
                    check(
                        spec,
                        sub,
                        item,
                        &format!("{path}[{index}]"),
                        problems,
                        depth + 1,
                    );
                }
            }
        }
        _ => {}
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec() -> Spec {
        Spec::read("https://api.contoso.example/openapi.json", json!({
            "openapi": "3.1.0", "info": {},
            "paths": {
                "/orders": {"post": {"requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/OrderIn"}}}},
                    "responses": {"201": {"description": "made", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Order"}}}}}}},
                "/orders/{id}": {"get": {"responses": {"200": {"description": "ok"}}}},
                "/orders/search": {"get": {"parameters": [{"in": "query", "name": "q", "required": true, "schema": {"type": "string"}}], "responses": {"200": {"description": "ok"}}}}
            },
            "components": {"schemas": {
                "OrderIn": {"type": "object", "additionalProperties": false, "required": ["customer_id", "currency"], "properties": {
                    "customer_id": {"type": "integer"}, "currency": {"type": "string", "enum": ["EUR", "USD"]},
                    "lines": {"type": "array", "items": {"type": "object", "required": ["sku"], "properties": {"sku": {"type": "string"}}}}}},
                "Order": {"type": "object", "required": ["id"], "properties": {"id": {"type": "integer"}}}
            }}
        })).unwrap()
    }

    #[test]
    fn a_body_is_held_to_its_schema_before_it_goes() {
        let spec = spec();
        let body =
            Body::Json(json!({"customer_id": "7", "currency": "GBP", "lines": [{}], "rush": true}));
        let (op, problems) =
            request(&spec, "POST", "https://api.contoso.example/orders", &body).unwrap();
        assert_eq!(op.path, "/orders");
        assert_eq!(
            problems,
            [
                "body.customer_id is string, not integer",
                "body.currency is \"GBP\", not one of EUR, USD",
                "body.lines[0].sku is required",
                "body.rush is not a field it takes",
            ]
        );
        let (_, none) = request(
            &spec,
            "POST",
            "https://api.contoso.example/orders",
            &Body::Json(json!({"customer_id": 7, "currency": "EUR"})),
        )
        .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn the_literal_path_wins_and_a_required_query_parameter_is_named() {
        let spec = spec();
        let (op, problems) = request(
            &spec,
            "GET",
            "https://api.contoso.example/orders/search",
            &Body::None,
        )
        .unwrap();
        assert_eq!(op.path, "/orders/search");
        assert_eq!(problems, ["the query parameter q is required"]);
        assert!(
            request(
                &spec,
                "PATCH",
                "https://api.contoso.example/orders",
                &Body::None
            )
            .is_err()
        );
    }

    #[test]
    fn an_answer_is_held_to_its_documented_status_and_schema() {
        let spec = spec();
        let op = spec
            .operations
            .iter()
            .find(|op| op.method == "POST")
            .unwrap();
        assert!(response(&spec, op, 201, r#"{"id": 601}"#).is_empty());
        assert_eq!(
            response(&spec, op, 201, r#"{"id": "x"}"#),
            ["response.id is string, not integer"]
        );
        assert_eq!(
            response(&spec, op, 500, "{}"),
            ["500 is not a status it documents (201)"]
        );
    }
}
