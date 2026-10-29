//! Swagger 2.0, OpenAPI 3.0 and 3.1 read into the one shape `Spec` holds:
//! servers resolved against where the spec came from, path parameters
//! shared, 2.0's body and form parameters made a request body.

use serde_json::{Map, Value};

use super::{METHODS, Operation, Param, RequestBody, ResponseDoc, Scheme, resolve};
use crate::url;

pub(super) fn servers_three(id: &str, raw: &Value) -> Vec<String> {
    raw["servers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|server| {
            let mut address = server["url"].as_str()?.to_owned();
            if let Some(variables) = server["variables"].as_object() {
                for (name, variable) in variables {
                    let default = variable["default"].as_str().unwrap_or_default();
                    address = address.replace(&format!("{{{name}}}"), default);
                }
            }
            Some(if address.contains("://") || !id.contains("://") {
                address
            } else {
                url::join(id, &address)
            })
        })
        .collect()
}

pub(super) fn servers_two(id: &str, raw: &Value) -> Vec<String> {
    let host = raw["host"].as_str().filter(|host| !host.is_empty());
    let base = raw["basePath"].as_str().unwrap_or("").trim_end_matches('/');
    let scheme = raw["schemes"]
        .as_array()
        .and_then(|schemes| {
            schemes
                .iter()
                .filter_map(Value::as_str)
                .find(|s| *s == "https")
                .or_else(|| schemes.first().and_then(Value::as_str))
        })
        .map(str::to_owned);
    match host {
        Some(host) => vec![format!(
            "{}://{host}{base}",
            scheme.as_deref().unwrap_or("https")
        )],
        None if id.contains("://") && !base.is_empty() => {
            vec![format!("{}{base}", url::origin(id))]
        }
        None => Vec::new(),
    }
}

pub(super) fn schemes_of(map: &Value) -> Vec<Scheme> {
    map.as_object()
        .into_iter()
        .flatten()
        .map(|(name, scheme)| {
            let kind = scheme["type"].as_str().unwrap_or("unknown");
            let (kind, detail) = match kind {
                "http" => (
                    scheme["scheme"]
                        .as_str()
                        .unwrap_or("http")
                        .to_ascii_lowercase(),
                    scheme["bearerFormat"].as_str().map(str::to_owned),
                ),
                "basic" => ("basic".to_owned(), None),
                "apiKey" => (
                    format!(
                        "apiKey {} {}",
                        scheme["in"].as_str().unwrap_or("header"),
                        scheme["name"].as_str().unwrap_or("?")
                    ),
                    None,
                ),
                "oauth2" => {
                    let flows = &scheme["flows"];
                    let token = flows
                        .as_object()
                        .and_then(|flows| flows.values().find_map(|flow| flow["tokenUrl"].as_str()))
                        .or_else(|| scheme["tokenUrl"].as_str())
                        .map(|token| format!("token {token}"));
                    ("oauth2".to_owned(), token)
                }
                "openIdConnect" => (
                    "openIdConnect".to_owned(),
                    scheme["openIdConnectUrl"].as_str().map(str::to_owned),
                ),
                other => (other.to_owned(), None),
            };
            Scheme {
                name: name.clone(),
                kind,
                detail,
            }
        })
        .collect()
}

/// A `security` list as the names it accepts: alternatives as separate
/// entries, schemes needed together joined by `+`. `None` when absent.
pub(super) fn requirement(value: &Value) -> Option<Vec<String>> {
    let list = value.as_array()?;
    Some(
        list.iter()
            .filter_map(Value::as_object)
            .filter(|together| !together.is_empty())
            .map(|together| together.keys().cloned().collect::<Vec<_>>().join("+"))
            .collect(),
    )
}

pub(super) fn operations(raw: &Value, two: bool) -> Vec<Operation> {
    let mut found = Vec::new();
    let consumes = strings(&raw["consumes"]);
    let produces = strings(&raw["produces"]);
    for (path, item) in raw["paths"].as_object().into_iter().flatten() {
        let Some(item) = resolve(raw, item) else {
            continue;
        };
        let shared = params(raw, &item["parameters"]);
        for method in METHODS {
            let Some(op) = item.get(method).filter(|op| op.is_object()) else {
                continue;
            };
            let mut parameters = shared.clone();
            for own in params(raw, &op["parameters"]) {
                parameters.retain(|p| !(p.name == own.name && p.location == own.location));
                parameters.push(own);
            }
            let mut body = None;
            if two {
                let consumes = Some(strings(&op["consumes"]))
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| consumes.clone());
                body = body_two(&mut parameters, &consumes);
            } else if let Some(request) = op.get("requestBody").and_then(|b| resolve(raw, b)) {
                body = Some(RequestBody {
                    required: request["required"].as_bool().unwrap_or(false),
                    content: content(&request["content"]),
                });
            }
            let produces = Some(strings(&op["produces"]))
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| produces.clone());
            let responses = op["responses"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(status, answer)| {
                    let answer = resolve(raw, answer)?;
                    let content = if two {
                        match answer.get("schema") {
                            Some(schema) => media(&produces)
                                .into_iter()
                                .map(|kind| (kind, schema.clone()))
                                .collect(),
                            None => Vec::new(),
                        }
                    } else {
                        content(&answer["content"])
                    };
                    Some(ResponseDoc {
                        status: status.clone(),
                        description: answer["description"].as_str().map(str::to_owned),
                        content,
                    })
                })
                .collect();
            let text = |key: &str| {
                op[key]
                    .as_str()
                    .map(str::to_owned)
                    .filter(|s| !s.is_empty())
            };
            found.push(Operation {
                method: method.to_ascii_uppercase(),
                path: path.clone(),
                operation_id: text("operationId"),
                summary: text("summary"),
                description: text("description"),
                tags: strings(&op["tags"]),
                deprecated: op["deprecated"].as_bool().unwrap_or(false),
                parameters,
                body,
                responses,
                security: requirement(&op["security"]),
            });
        }
    }
    found
}

pub(super) fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

pub(super) fn media(types: &[String]) -> Vec<String> {
    if types.is_empty() {
        vec!["application/json".to_owned()]
    } else {
        types.to_vec()
    }
}

pub(super) fn content(value: &Value) -> Vec<(String, Value)> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(kind, entry)| {
            (
                kind.clone(),
                entry.get("schema").cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

pub(super) fn params(raw: &Value, list: &Value) -> Vec<Param> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|param| {
            let param = resolve(raw, param)?;
            let location = param["in"].as_str()?.to_owned();
            // Swagger 2.0 puts the schema's words on the parameter itself.
            let schema = param.get("schema").cloned().unwrap_or_else(|| {
                let mut schema = Map::new();
                for key in [
                    "type",
                    "format",
                    "items",
                    "enum",
                    "default",
                    "minimum",
                    "maximum",
                    "maxLength",
                    "minLength",
                    "pattern",
                    "collectionFormat",
                ] {
                    if let Some(value) = param.get(key) {
                        schema.insert(key.to_owned(), value.clone());
                    }
                }
                Value::Object(schema)
            });
            Some(Param {
                name: param["name"].as_str()?.to_owned(),
                required: param["required"].as_bool().unwrap_or(location == "path"),
                location,
                schema,
                description: param["description"].as_str().map(str::to_owned),
                deprecated: param["deprecated"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// Swagger 2.0's `body` and `formData` parameters, taken out of
/// `parameters` and made the request body OpenAPI 3 would have.
pub(super) fn body_two(parameters: &mut Vec<Param>, consumes: &[String]) -> Option<RequestBody> {
    if let Some(at) = parameters.iter().position(|p| p.location == "body") {
        let param = parameters.remove(at);
        return Some(RequestBody {
            required: param.required,
            content: media(consumes)
                .into_iter()
                .map(|kind| (kind, param.schema.clone()))
                .collect(),
        });
    }
    let form: Vec<Param> = parameters
        .iter()
        .filter(|p| p.location == "formData")
        .cloned()
        .collect();
    if form.is_empty() {
        return None;
    }
    parameters.retain(|p| p.location != "formData");
    let file = form.iter().any(|p| p.schema["type"] == "file");
    let kind = if file || consumes.iter().any(|c| c.contains("multipart")) {
        "multipart/form-data"
    } else {
        "application/x-www-form-urlencoded"
    };
    let properties: Map<String, Value> = form
        .iter()
        .map(|p| (p.name.clone(), p.schema.clone()))
        .collect();
    let required: Vec<Value> = form
        .iter()
        .filter(|p| p.required)
        .map(|p| Value::String(p.name.clone()))
        .collect();
    Some(RequestBody {
        required: !required.is_empty(),
        content: vec![(
            kind.to_owned(),
            serde_json::json!({"type": "object", "properties": properties, "required": required}),
        )],
    })
}
