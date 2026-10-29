//! YAML specs (Quarkus, drf-spectacular and most files kept in repositories)
//! read into the JSON value a JSON spec parses to. Keys are written as text,
//! since a spec's responses are keyed `200:`, which YAML reads as a number.

use anyhow::{Result, anyhow};
use saphyr::{LoadableYamlNode, Scalar, Yaml};
use serde_json::{Map, Number, Value};

/// The first document in `text`.
pub(crate) fn parse(text: &str) -> Result<Value> {
    let documents = Yaml::load_from_str(text).map_err(|why| anyhow!("not YAML: {why}"))?;
    let first = documents
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("an empty YAML document"))?;
    Ok(value(first))
}

fn value(node: Yaml<'_>) -> Value {
    match node {
        Yaml::Value(scalar) => scalar_value(scalar),
        Yaml::Representation(text, ..) => Value::String(text.into_owned()),
        Yaml::Sequence(items) => Value::Array(items.into_iter().map(value).collect()),
        Yaml::Mapping(map) => Value::Object(
            map.into_iter()
                .map(|(key, item)| (key_text(key), value(item)))
                .collect::<Map<_, _>>(),
        ),
        Yaml::Tagged(_, inner) => value(*inner),
        Yaml::Alias(_) | Yaml::BadValue => Value::Null,
    }
}

fn scalar_value(scalar: Scalar<'_>) -> Value {
    match scalar {
        Scalar::Null => Value::Null,
        Scalar::Boolean(flag) => Value::Bool(flag),
        Scalar::Integer(number) => Value::Number(number.into()),
        Scalar::FloatingPoint(number) => {
            Number::from_f64(number.into_inner()).map_or(Value::Null, Value::Number)
        }
        Scalar::String(text) => Value::String(text.into_owned()),
    }
}

fn key_text(key: Yaml<'_>) -> String {
    match value(key) {
        Value::String(text) => text,
        Value::Null => "null".to_owned(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse;

    #[test]
    fn a_yaml_spec_reads_as_its_json_with_numeric_keys_as_text() {
        let text = "openapi: 3.1.0\ninfo:\n  title: orders API\n  version: 1.0.0\npaths:\n  /orders:\n    get:\n      responses:\n        200:\n          description: OK\n        \"404\":\n          description: gone\n      deprecated: false\n      x-rate: 1.5\n";
        assert_eq!(
            parse(text).unwrap(),
            json!({"openapi": "3.1.0", "info": {"title": "orders API", "version": "1.0.0"},
                "paths": {"/orders": {"get": {"responses": {"200": {"description": "OK"}, "404": {"description": "gone"}}, "deprecated": false, "x-rate": 1.5}}}})
        );
        assert!(parse("a: [unclosed").is_err());
    }
}
