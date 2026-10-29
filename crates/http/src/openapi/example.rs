//! A body to start from, built from an operation's schema: its required
//! fields (every field when none is required), each a value of its type.

use serde_json::{Map, Value, json};

use super::Spec;
use super::fields::Node;

const DEPTH: usize = 4;

pub(crate) fn example(spec: &Spec, schema: &Value) -> Value {
    build(spec, schema, 0, &mut Vec::new())
}

fn build(spec: &Spec, schema: &Value, depth: usize, stack: &mut Vec<String>) -> Value {
    let node = Node::of(spec, schema);
    if let Some(given) = node
        .raw
        .get("example")
        .cloned()
        .or_else(|| {
            node.raw["examples"]
                .as_array()
                .and_then(|e| e.first().cloned())
        })
        .or_else(|| node.default.clone())
        .or_else(|| node.values.first().cloned())
        .or_else(|| node.raw.get("const").cloned())
    {
        return given;
    }
    let recursive = node.name.as_ref().is_some_and(|name| stack.contains(name));
    if depth >= DEPTH || recursive {
        return Value::Null;
    }
    if let Some(name) = &node.name {
        stack.push(name.clone());
    }
    let value = match node.kind.as_str() {
        "object" => {
            let wanted: Vec<&(String, Value)> = if node.required.is_empty() {
                node.properties.iter().take(12).collect()
            } else {
                node.properties
                    .iter()
                    .filter(|(key, _)| node.required.contains(key))
                    .collect()
            };
            let mut object = Map::new();
            for (key, sub) in wanted {
                let sub_node = Node::of(spec, sub);
                if sub_node.read_only {
                    continue;
                }
                object.insert(key.clone(), build(spec, sub, depth + 1, stack));
            }
            Value::Object(object)
        }
        "array" => match &node.items {
            Some(items) => json!([build(spec, items, depth + 1, stack)]),
            None => json!([]),
        },
        "map" => match &node.additional {
            Some(value @ Value::Object(_)) => json!({"key": build(spec, value, depth + 1, stack)}),
            _ => json!({}),
        },
        "oneOf" => node
            .variants
            .first()
            .map_or(Value::Null, |first| build(spec, first, depth + 1, stack)),
        "integer" => json!(0),
        "number" => json!(0.0),
        "boolean" => json!(false),
        "string" => Value::String(
            match node.format.as_deref() {
                Some("date-time") => "2026-01-01T00:00:00Z",
                Some("date") => "2026-01-01",
                Some("uuid") => "00000000-0000-0000-0000-000000000000",
                Some("email") => "someone@contoso.example",
                Some("uri" | "url") => "https://contoso.example/",
                Some("byte") => "",
                _ => "string",
            }
            .to_owned(),
        ),
        _ => Value::Null,
    };
    if node.name.is_some() {
        stack.pop();
    }
    value
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_example_holds_the_required_fields_each_a_value_of_its_type() {
        let spec = Spec::read("s.json", json!({"openapi": "3.0.3", "info": {}, "paths": {}, "components": {"schemas": {
            "OrderIn": {"type": "object", "required": ["customer_id", "lines", "currency"], "properties": {
                "customer_id": {"type": "integer"},
                "currency": {"type": "string", "enum": ["EUR", "USD"]},
                "lines": {"type": "array", "items": {"type": "object", "required": ["sku"], "properties": {"sku": {"type": "string", "example": "SKU-1"}, "qty": {"type": "integer"}}}},
                "note": {"type": "string"},
                "id": {"type": "integer", "readOnly": true}
            }}
        }}})).unwrap();
        assert_eq!(
            example(&spec, &json!({"$ref": "#/components/schemas/OrderIn"})),
            json!({"customer_id": 0, "lines": [{"sku": "SKU-1"}], "currency": "EUR"})
        );
    }
}
