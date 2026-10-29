//! A schema as field paths (`lines[].sku  string  required`): `$ref`s
//! followed, `allOf` merged, a nullable `anyOf` read as the one type it
//! allows, recursion marked rather than followed.

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::Spec;

/// How deep fields are followed.
const DEPTH: usize = 6;

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct Field {
    /// `customer_id`, `lines[].sku`, `meta{}.note`; `[]` for a list body.
    pub(crate) path: String,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) required: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) nullable: bool,
    #[serde(rename = "enum", skip_serializing_if = "Vec::is_empty")]
    pub(crate) values: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) default: Option<Value>,
    /// `maxLength 40`, `minimum 1`, `pattern ^[A-Z]{3}$`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) limits: Vec<String>,
    /// The component it is, when it is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) read_only: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) deprecated: bool,
}

/// One schema read for what it allows, its refs followed.
#[derive(Clone, Debug, Default)]
pub(crate) struct Node {
    /// `string`, `integer`, `object`, `array`, `map`, `oneOf`, `any`.
    pub(crate) kind: String,
    pub(crate) name: Option<String>,
    pub(crate) format: Option<String>,
    pub(crate) nullable: bool,
    pub(crate) values: Vec<Value>,
    pub(crate) default: Option<Value>,
    pub(crate) limits: Vec<(String, Value)>,
    pub(crate) description: Option<String>,
    pub(crate) properties: Vec<(String, Value)>,
    pub(crate) required: Vec<String>,
    pub(crate) items: Option<Value>,
    /// A map's values, or `true` for any.
    pub(crate) additional: Option<Value>,
    /// `false` closes the object to fields it does not name.
    pub(crate) closed: bool,
    pub(crate) variants: Vec<Value>,
    pub(crate) discriminator: Option<String>,
    pub(crate) read_only: bool,
    pub(crate) deprecated: bool,
    /// The schema as written, refs followed, for its `example`.
    pub(crate) raw: Value,
}

const LIMITS: [&str; 9] = [
    "maxLength",
    "minLength",
    "maximum",
    "minimum",
    "exclusiveMaximum",
    "exclusiveMinimum",
    "pattern",
    "maxItems",
    "minItems",
];

impl Node {
    /// What `schema` allows, through its refs, `allOf` and a nullable union.
    pub(crate) fn of(spec: &Spec, schema: &Value) -> Self {
        Self::read(spec, schema, 0)
    }

    fn read(spec: &Spec, schema: &Value, depth: usize) -> Self {
        let name = Spec::ref_name(schema);
        let Some(schema) = spec.deref(schema) else {
            return Self {
                kind: format!(
                    "ref {} (not in this spec)",
                    schema["$ref"].as_str().unwrap_or("?")
                ),
                name,
                ..Self::default()
            };
        };
        let mut node = Self {
            name,
            raw: schema.clone(),
            format: schema["format"].as_str().map(str::to_owned),
            nullable: schema["nullable"].as_bool().unwrap_or(false)
                || schema["x-nullable"].as_bool().unwrap_or(false),
            values: schema["enum"].as_array().cloned().unwrap_or_default(),
            default: schema.get("default").cloned(),
            description: schema["description"]
                .as_str()
                .or_else(|| schema["title"].as_str())
                .map(|text| cut(text, 160)),
            required: schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            items: schema.get("items").cloned(),
            read_only: schema["readOnly"].as_bool().unwrap_or(false),
            deprecated: schema["deprecated"].as_bool().unwrap_or(false),
            discriminator: schema["discriminator"]["propertyName"]
                .as_str()
                .or_else(|| schema["discriminator"].as_str())
                .map(str::to_owned),
            ..Self::default()
        };
        node.limits = LIMITS
            .iter()
            .filter_map(|limit| {
                schema
                    .get(*limit)
                    .map(|value| ((*limit).to_owned(), value.clone()))
            })
            .collect();
        if let Some(properties) = schema["properties"].as_object() {
            node.properties = properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
        }
        match schema.get("additionalProperties") {
            Some(Value::Bool(false)) => node.closed = true,
            Some(Value::Bool(true)) => node.additional = Some(Value::Bool(true)),
            Some(other @ Value::Object(_)) => node.additional = Some(other.clone()),
            _ => {}
        }
        let mut kinds: Vec<String> = match &schema["type"] {
            Value::String(kind) => vec![kind.clone()],
            Value::Array(kinds) => kinds
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        if kinds.iter().any(|kind| kind == "null") {
            node.nullable = true;
            kinds.retain(|kind| kind != "null");
        }
        if node.values.iter().any(Value::is_null) {
            node.nullable = true;
            node.values.retain(|value| !value.is_null());
        }
        node.kind = kinds.join("|");
        if depth < DEPTH {
            for part in schema["allOf"].as_array().into_iter().flatten() {
                let part = Self::read(spec, part, depth + 1);
                node.merge(part);
            }
        }
        let union = schema["oneOf"]
            .as_array()
            .or_else(|| schema["anyOf"].as_array());
        if let Some(union) = union {
            let (nulls, others): (Vec<&Value>, Vec<&Value>) = union.iter().partition(|v| {
                v["type"] == "null"
                    || (v.get("enum").is_some() && v["enum"] == Value::Array(vec![Value::Null]))
            });
            if !nulls.is_empty() {
                node.nullable = true;
            }
            if others.len() == 1 && depth < DEPTH {
                let only = Self::read(spec, others[0], depth + 1);
                let description = node.description.take();
                node.merge(only);
                node.description = description.or(node.description.take());
            } else {
                node.variants = others.into_iter().cloned().collect();
            }
        }
        if node.kind.is_empty() {
            node.kind = if !node.variants.is_empty() {
                "oneOf".to_owned()
            } else if !node.properties.is_empty() || node.closed {
                "object".to_owned()
            } else if node.additional.is_some() {
                "map".to_owned()
            } else if node.items.is_some() {
                "array".to_owned()
            } else if let Some(first) = node.values.first() {
                match first {
                    Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
                    Value::Number(_) => "number",
                    Value::Bool(_) => "boolean",
                    _ => "string",
                }
                .to_owned()
            } else {
                "any".to_owned()
            };
        }
        if node.kind == "object" && node.properties.is_empty() && node.additional.is_some() {
            node.kind = "map".to_owned();
        }
        node
    }

    /// `part`'s fields and words added to this node's (`allOf`).
    fn merge(&mut self, part: Self) {
        if self.kind.is_empty() || self.kind == "any" {
            self.kind = part.kind;
        }
        if self.name.is_none() && self.properties.is_empty() {
            self.name = part.name;
        }
        for (key, value) in part.properties {
            if !self.properties.iter().any(|(held, _)| *held == key) {
                self.properties.push((key, value));
            }
        }
        for key in part.required {
            if !self.required.contains(&key) {
                self.required.push(key);
            }
        }
        self.format = self.format.take().or(part.format);
        self.nullable |= part.nullable;
        if self.values.is_empty() {
            self.values = part.values;
        }
        self.default = self.default.take().or(part.default);
        self.description = self.description.take().or(part.description);
        self.items = self.items.take().or(part.items);
        self.additional = self.additional.take().or(part.additional);
        self.closed |= part.closed;
        if self.variants.is_empty() {
            self.variants = part.variants;
        }
        self.limits.extend(part.limits);
        if self.raw.get("example").is_none()
            && let Some(example) = part.raw.get("example")
        {
            self.raw["example"] = example.clone();
        }
    }

    /// What a field of this type is called: its component's name, else its
    /// type.
    fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.kind.clone())
    }

    fn has_fields(&self) -> bool {
        !self.properties.is_empty()
    }
}

/// Every field `schema` holds, outermost first.
pub(crate) fn flatten(spec: &Spec, schema: &Value) -> Vec<Field> {
    let mut walk = Walk {
        spec,
        stack: Vec::new(),
        out: Vec::new(),
    };
    let root = Node::of(spec, schema);
    match root.kind.as_str() {
        "object" if root.has_fields() || root.variants.is_empty() => walk.children(&root, "", 0),
        _ => walk.field(schema, "", false, 0),
    }
    walk.out
}

struct Walk<'s> {
    spec: &'s Spec,
    stack: Vec<String>,
    out: Vec<Field>,
}

impl Walk<'_> {
    fn children(&mut self, node: &Node, prefix: &str, depth: usize) {
        let pushed = node
            .name
            .clone()
            .inspect(|name| self.stack.push(name.clone()));
        for (key, schema) in &node.properties {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            self.field(schema, &path, node.required.contains(key), depth + 1);
        }
        if pushed.is_some() {
            self.stack.pop();
        }
    }

    fn field(&mut self, schema: &Value, path: &str, required: bool, depth: usize) {
        let node = Node::of(self.spec, schema);
        let mut field = describe(&node, path, required);
        let recursive = node
            .name
            .as_ref()
            .is_some_and(|name| self.stack.contains(name));
        let deep = depth >= DEPTH;
        match node.kind.as_str() {
            "array" => {
                let item = node
                    .items
                    .as_ref()
                    .map(|items| Node::of(self.spec, items))
                    .unwrap_or_default();
                field.kind = format!(
                    "array of {}",
                    if item.kind.is_empty() {
                        "any".to_owned()
                    } else {
                        item.label()
                    }
                );
                if field.values.is_empty() {
                    field.values = texts(&item.values);
                }
                field.format = field.format.or(item.format.clone());
                let (shown, prefix) = if path.is_empty() {
                    ("[]".to_owned(), "[]".to_owned())
                } else {
                    (path.to_owned(), format!("{path}[]"))
                };
                field.path = shown;
                let item_recursive = item
                    .name
                    .as_ref()
                    .is_some_and(|name| self.stack.contains(name));
                if item.has_fields() && item_recursive {
                    field.kind.push_str(" (recursive)");
                }
                self.out.push(field);
                if item.has_fields() && !item_recursive && !deep {
                    self.children(&item, &prefix, depth);
                }
            }
            "map" => {
                let value = match &node.additional {
                    Some(Value::Object(_)) => node
                        .additional
                        .as_ref()
                        .map(|v| Node::of(self.spec, v))
                        .unwrap_or_default(),
                    _ => Node {
                        kind: "any".to_owned(),
                        ..Node::default()
                    },
                };
                field.kind = format!("map of {}", value.label());
                self.out.push(field);
                if value.has_fields() && !deep {
                    self.children(&value, &format!("{path}{{}}"), depth);
                }
            }
            "oneOf" => {
                let names: Vec<String> = node
                    .variants
                    .iter()
                    .map(|v| Node::of(self.spec, v).label())
                    .collect();
                field.kind = format!("oneOf {}", names.join(" | "));
                if let Some(by) = &node.discriminator {
                    field.kind.push_str(&format!(" (by {by})"));
                }
                self.out.push(field);
            }
            "object" if node.has_fields() => {
                if recursive {
                    field.kind.push_str(" (recursive)");
                }
                if deep && !recursive {
                    field.kind.push_str(" (deeper fields left out)");
                }
                if !path.is_empty() {
                    self.out.push(field);
                }
                if !recursive && !deep {
                    self.children(&node, path, depth);
                }
            }
            _ => self.out.push(field),
        }
    }
}

fn describe(node: &Node, path: &str, required: bool) -> Field {
    Field {
        path: path.to_owned(),
        kind: node.kind.clone(),
        format: node.format.clone(),
        required,
        nullable: node.nullable,
        values: texts(&node.values),
        default: node.default.clone(),
        limits: node
            .limits
            .iter()
            .map(|(name, value)| format!("{name} {}", text(value)))
            .collect(),
        schema: node.name.clone(),
        description: node.description.clone(),
        read_only: node.read_only,
        deprecated: node.deprecated,
    }
}

pub(crate) fn texts(values: &[Value]) -> Vec<String> {
    values.iter().map(text).collect()
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn cut(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    format!("{}…", text.chars().take(max).collect::<String>())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec(schemas: Value) -> Spec {
        Spec::read("s.json", json!({"openapi": "3.1.0", "info": {}, "paths": {}, "components": {"schemas": schemas}})).unwrap()
    }

    #[test]
    fn a_schema_flattens_to_field_paths_through_refs_and_arrays() {
        let spec = spec(json!({
            "OrderIn": {"type": "object", "required": ["customer_id", "lines"], "properties": {
                "customer_id": {"type": "integer"},
                "lines": {"type": "array", "items": {"$ref": "#/components/schemas/Line"}},
                "note": {"anyOf": [{"type": "string", "maxLength": 200}, {"type": "null"}]},
                "currency": {"type": "string", "enum": ["EUR", "USD"]}
            }},
            "Line": {"type": "object", "required": ["sku"], "properties": {"sku": {"type": "string"}, "qty": {"type": "integer", "default": 1}}}
        }));
        let fields = flatten(&spec, &json!({"$ref": "#/components/schemas/OrderIn"}));
        let shown: Vec<(String, String, bool)> = fields
            .iter()
            .map(|f| (f.path.clone(), f.kind.clone(), f.required))
            .collect();
        assert_eq!(
            shown,
            [
                ("customer_id".into(), "integer".into(), true),
                ("lines".into(), "array of Line".into(), true),
                ("lines[].sku".into(), "string".into(), true),
                ("lines[].qty".into(), "integer".into(), false),
                ("note".into(), "string".into(), false),
                ("currency".into(), "string".into(), false),
            ]
        );
        assert!(fields[4].nullable);
        assert_eq!(fields[4].limits, ["maxLength 200"]);
        assert_eq!(fields[5].values, ["EUR", "USD"]);
    }

    #[test]
    fn all_of_merges_and_recursion_is_marked_not_followed() {
        let spec = spec(json!({
            "Base": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string", "format": "uuid"}}},
            "Node": {"allOf": [{"$ref": "#/components/schemas/Base"}, {"type": "object", "properties": {"children": {"type": "array", "items": {"$ref": "#/components/schemas/Node"}}}}]}
        }));
        let fields = flatten(&spec, &json!({"$ref": "#/components/schemas/Node"}));
        let shown: Vec<(String, String)> = fields
            .iter()
            .map(|f| (f.path.clone(), f.kind.clone()))
            .collect();
        assert_eq!(
            shown,
            [
                ("id".into(), "string".into()),
                ("children".into(), "array of Node (recursive)".into())
            ]
        );
        assert!(fields[0].required);
        let list = flatten(
            &spec,
            &json!({"type": "array", "items": {"$ref": "#/components/schemas/Base"}}),
        );
        assert_eq!(list[0].path, "[]");
        assert_eq!(list[1].path, "[].id");
    }
}
