//! A spec as `spec diff` compares it: operations keyed by `METHOD PATH`,
//! their parameters, request fields and responses keyed by name, so core's
//! diff names each change by where it is; and which changes break the
//! clients of the left side.

use agent_cli_core::{Change, Rules};
use serde_json::{Map, Value, json};

use super::fields::{Field, flatten};
use super::{Operation, Spec};

pub(crate) const RULES: Rules = Rules {
    keys: &[
        ("operations", "key"),
        ("operations.*.parameters", "key"),
        ("operations.*.request.fields", "key"),
        ("operations.*.responses", "key"),
        ("operations.*.responses.*.fields", "key"),
    ],
    noise: &["version", "operations.*.summary", "operations.*.path"],
    breaking: Some(breaking),
    ..Rules::PLAIN
};

/// The row a side of `spec diff` is.
pub(crate) fn row(spec: &Spec) -> Value {
    let servers: Map<String, Value> = spec
        .servers
        .iter()
        .map(|s| (s.clone(), json!(true)))
        .collect();
    let operations: Vec<Value> = spec
        .operations
        .iter()
        .map(|op| operation(spec, op))
        .collect();
    json!({
        "title": spec.title,
        "version": spec.version,
        "servers": servers,
        "operations": operations,
    })
}

fn operation(spec: &Spec, op: &Operation) -> Value {
    let needs = op.needs(spec);
    let security = if needs.is_empty() {
        "none".to_owned()
    } else {
        needs.join(" or ")
    };
    let parameters: Vec<Value> = op
        .parameters
        .iter()
        .map(|param| {
            let mut field = flatten(spec, &param.schema)
                .into_iter()
                .next()
                .unwrap_or_else(|| Field {
                    path: String::new(),
                    kind: "any".to_owned(),
                    format: None,
                    required: false,
                    nullable: false,
                    values: Vec::new(),
                    default: None,
                    limits: Vec::new(),
                    schema: None,
                    description: None,
                    read_only: false,
                    deprecated: false,
                });
            field.required = param.required;
            attributes(&format!("{} {}", param.location, param.name), &field)
        })
        .collect();
    let request = op.body.as_ref().map(|body| {
        let content: Map<String, Value> = body
            .content
            .iter()
            .map(|(media, _)| (media.clone(), json!(true)))
            .collect();
        let fields: Vec<Value> = op
            .body_schema()
            .map(|schema| flatten(spec, schema))
            .unwrap_or_default()
            .iter()
            .map(|field| attributes(&field.path, field))
            .collect();
        json!({"required": body.required, "content": content, "fields": fields})
    });
    let responses: Vec<Value> = op
        .responses
        .iter()
        .map(|doc| {
            let content: Map<String, Value> = doc
                .content
                .iter()
                .map(|(media, _)| (media.clone(), json!(true)))
                .collect();
            let schema = doc
                .content
                .iter()
                .find(|(media, _)| media.contains("json"))
                .or_else(|| doc.content.first());
            let fields: Vec<Value> = schema
                .map(|(_, schema)| flatten(spec, schema))
                .unwrap_or_default()
                .iter()
                .map(|field| attributes(&field.path, field))
                .collect();
            json!({"key": doc.status, "content": content, "fields": fields})
        })
        .collect();
    json!({
        "key": op.key(),
        "path": op.path,
        "operation_id": op.operation_id,
        "summary": op.summary,
        "deprecated": op.deprecated,
        "security": security,
        "parameters": parameters,
        "request": request,
        "responses": responses,
    })
}

fn attributes(key: &str, field: &Field) -> Value {
    let values: Map<String, Value> = field
        .values
        .iter()
        .map(|v| (v.clone(), json!(true)))
        .collect();
    let limits: Map<String, Value> = field
        .limits
        .iter()
        .filter_map(|limit| limit.split_once(' '))
        .map(|(name, value)| {
            (
                name.to_owned(),
                serde_json::from_str(value).unwrap_or_else(|_| json!(value)),
            )
        })
        .collect();
    let mut row = json!({"key": key, "type": field.kind, "required": field.required, "nullable": field.nullable});
    if let Some(format) = &field.format {
        row["format"] = json!(format);
    }
    if !values.is_empty() {
        row["enum"] = Value::Object(values);
    }
    if !limits.is_empty() {
        row["limits"] = Value::Object(limits);
    }
    row
}

/// The parts of a change's path: the operation, then what of it.
const PARTS: [&str; 8] = [
    "parameters",
    "request",
    "responses",
    "security",
    "deprecated",
    "operation_id",
    "path",
    "summary",
];

/// Why a change breaks a client written against the left spec.
fn breaking(change: &Change) -> Option<String> {
    let rest = change.path.strip_prefix("operations.")?;
    let (op, tail) = match PARTS
        .iter()
        .filter_map(|part| {
            let inner = rest.find(&format!(".{part}."));
            let end = rest
                .ends_with(&format!(".{part}"))
                .then(|| rest.len() - part.len() - 1);
            inner.or(end)
        })
        .min()
    {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => (rest, ""),
    };
    let (left, right) = (change.left.as_ref(), change.right.as_ref());
    let removed = left.is_some() && right.is_none();
    let added = left.is_none() && right.is_some();
    // Core's keyed diff moves an item's key into the path, so a whole
    // parameter or field is the object holding its type and required.
    let whole = |value: Option<&Value>| {
        value.is_some_and(|v| v.get("type").is_some() && v.get("required").is_some())
    };
    let truth = |value: Option<&Value>| value.and_then(Value::as_bool).unwrap_or(false);
    if tail.is_empty() {
        return removed.then(|| format!("{op} is gone"));
    }
    if tail == "security" {
        return match (left.and_then(Value::as_str), right.and_then(Value::as_str)) {
            (Some("none"), Some(now)) => Some(format!("{op} now needs a sign-in ({now})")),
            (Some(before), Some(now)) if now != "none" => {
                Some(format!("{op}'s sign-in changed from {before} to {now}"))
            }
            _ => None,
        };
    }
    if let Some(param) = tail.strip_prefix("parameters.") {
        if whole(left) || whole(right) {
            return (added && right.is_some_and(|v| v["required"] == true))
                .then(|| format!("{op} has a new required parameter {param}"));
        }
        return narrowed(&format!("{op}'s parameter"), param, change, true);
    }
    if tail == "request" {
        return (added && right.is_some_and(|v| v["required"] == true))
            .then(|| format!("{op} now requires a body"));
    }
    if tail == "request.required" {
        return (!truth(left) && truth(right)).then(|| format!("{op} now requires a body"));
    }
    if let Some(media) = tail.strip_prefix("request.content.") {
        return removed.then(|| format!("{op} no longer takes a {media} body"));
    }
    if let Some(field) = tail.strip_prefix("request.fields.") {
        if whole(left) || whole(right) {
            return (added && right.is_some_and(|v| v["required"] == true))
                .then(|| format!("{op}'s body has a new required field {field}"));
        }
        return narrowed(&format!("{op}'s body field"), field, change, true);
    }
    if let Some(answer) = tail.strip_prefix("responses.") {
        let (status, rest) = answer.split_once('.').unwrap_or((answer, ""));
        if rest.is_empty() {
            return (removed && status.starts_with('2'))
                .then(|| format!("{op} no longer answers {status}"));
        }
        if let Some(media) = rest.strip_prefix("content.") {
            return removed.then(|| format!("{op}'s {status} answer is no longer {media}"));
        }
        if let Some(field) = rest.strip_prefix("fields.") {
            if whole(left) || whole(right) {
                return removed.then(|| format!("{op}'s {status} answer lost the field {field}"));
            }
            return narrowed(
                &format!("{op}'s {status} answer field"),
                field,
                change,
                false,
            );
        }
        return None;
    }
    None
}

/// For a parameter's or field's attribute (`currency.enum.GBP`,
/// `customer_id.type`): whether the change breaks a client that sends it
/// (`sent`) or reads it.
fn narrowed(what: &str, key_and_attr: &str, change: &Change, sent: bool) -> Option<String> {
    let (left, right) = (change.left.as_ref(), change.right.as_ref());
    let text = |value: Option<&Value>| {
        value.map_or_else(
            || "nothing".to_owned(),
            |v| v.as_str().map_or_else(|| v.to_string(), str::to_owned),
        )
    };
    if let Some(at) = key_and_attr.rfind(".enum.") {
        let (key, value) = (&key_and_attr[..at], &key_and_attr[at + 6..]);
        return (sent && left.is_some() && right.is_none())
            .then(|| format!("{what} {key} no longer takes {value}"));
    }
    if let Some(key) = key_and_attr.strip_suffix(".enum") {
        return (sent && left.is_none())
            .then(|| format!("{what} {key} now takes only some values"));
    }
    if let Some(at) = key_and_attr.rfind(".limits.") {
        let (key, limit) = (&key_and_attr[..at], &key_and_attr[at + 8..]);
        let (before, after) = (left.and_then(Value::as_f64), right.and_then(Value::as_f64));
        let tighter = match (limit, before, after) {
            (_, None, Some(_)) => true,
            ("maxLength" | "maximum" | "exclusiveMaximum" | "maxItems", Some(b), Some(a)) => a < b,
            ("minLength" | "minimum" | "exclusiveMinimum" | "minItems", Some(b), Some(a)) => a > b,
            ("pattern", ..) => left != right && right.is_some(),
            _ => false,
        };
        return (sent && tighter).then(|| {
            format!(
                "{what} {key}'s {limit} narrowed from {} to {}",
                text(left),
                text(right)
            )
        });
    }
    let (key, attr) = key_and_attr.rsplit_once('.')?;
    let truth = |value: Option<&Value>| value.and_then(Value::as_bool).unwrap_or(false);
    match attr {
        "type" | "format" if left.is_some() && right.is_some() => Some(format!(
            "{what} {key}'s {attr} changed from {} to {}",
            text(left),
            text(right)
        )),
        "format" if sent && left.is_none() => {
            Some(format!("{what} {key} now needs the format {}", text(right)))
        }
        "required" if sent && !truth(left) && truth(right) => {
            Some(format!("{what} {key} is now required"))
        }
        "required" if !sent && truth(left) && !truth(right) => {
            Some(format!("{what} {key} may now be missing"))
        }
        "nullable" if sent && truth(left) && !truth(right) => {
            Some(format!("{what} {key} may no longer be null"))
        }
        "nullable" if !sent && !truth(left) && truth(right) => {
            Some(format!("{what} {key} may now be null"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn change(path: &str, left: Option<Value>, right: Option<Value>) -> Change {
        Change {
            path: path.to_owned(),
            left,
            right,
            redacted: false,
            same: None,
            lines: Vec::new(),
            breaking: None,
        }
    }

    #[test]
    fn what_breaks_a_client_is_named_and_what_does_not_is_not() {
        let op = json!({"key": "DELETE /orders/{}"});
        assert_eq!(
            breaking(&change(
                "operations.DELETE /orders/{}",
                Some(op.clone()),
                None
            ))
            .as_deref(),
            Some("DELETE /orders/{} is gone")
        );
        assert_eq!(
            breaking(&change(
                "operations.GET /orders/{}/fulfilment",
                None,
                Some(op)
            )),
            None
        );
        let required = json!({"type": "string", "required": true});
        assert_eq!(
            breaking(&change(
                "operations.POST /orders.request.fields.currency",
                None,
                Some(required)
            ))
            .as_deref(),
            Some("POST /orders's body has a new required field currency")
        );
        let optional = json!({"type": "integer", "required": false});
        assert_eq!(
            breaking(&change(
                "operations.GET /orders.parameters.query customer_id",
                None,
                Some(optional)
            )),
            None
        );
        assert_eq!(
            breaking(&change(
                "operations.GET /orders.parameters.query status.enum.on_hold",
                Some(json!(true)),
                None
            ))
            .as_deref(),
            Some("GET /orders's parameter query status no longer takes on_hold")
        );
        assert_eq!(
            breaking(&change(
                "operations.GET /orders.responses.200.fields.[].note",
                Some(json!({"type": "string", "required": false})),
                None
            ))
            .as_deref(),
            Some("GET /orders's 200 answer lost the field [].note")
        );
        assert_eq!(
            breaking(&change(
                "operations.GET /orders.responses.200.fields.[].note",
                None,
                Some(json!({"type": "string", "required": false}))
            )),
            None
        );
        assert_eq!(
            breaking(&change(
                "operations.POST /orders.request.fields.note.limits.maxLength",
                Some(json!(200)),
                Some(json!(100))
            ))
            .as_deref(),
            Some("POST /orders's body field note's maxLength narrowed from 200 to 100")
        );
        assert_eq!(
            breaking(&change(
                "operations.GET /orders.security",
                Some(json!("none")),
                Some(json!("bearer"))
            ))
            .as_deref(),
            Some("GET /orders now needs a sign-in (bearer)")
        );
        assert_eq!(
            breaking(&change(
                "operations.GET /orders.deprecated",
                Some(json!(false)),
                Some(json!(true))
            )),
            None
        );
        assert_eq!(
            breaking(&change(
                "operations.GET /orders/{}.responses.200.fields.total.type",
                Some(json!("integer")),
                Some(json!("string"))
            ))
            .as_deref(),
            Some("GET /orders/{}'s 200 answer field total's type changed from integer to string")
        );
    }
}
