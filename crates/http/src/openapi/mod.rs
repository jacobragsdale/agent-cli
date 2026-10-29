//! OpenAPI specs: Swagger 2.0, OpenAPI 3.0 and 3.1 read into one shape that
//! every spec, operation and schema verb reads, so none of them knows which
//! it was given.

pub(crate) mod check;
pub(crate) mod compare;
pub(crate) mod example;
pub(crate) mod fields;
pub(crate) mod find;
pub(crate) mod load;
mod read;
mod yaml;

use agent_cli_core::Failure;
use anyhow::Result;
use serde_json::Value;

use crate::url;

pub(crate) const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// A spec as every verb reads it.
pub(crate) struct Spec {
    /// The URL it was read from, or the file's path: what `spec get` prints
    /// as `id` and the other verbs take.
    pub(crate) id: String,
    pub(crate) raw: Value,
    /// `OpenAPI 3.1.0`, `Swagger 2.0`.
    pub(crate) format: String,
    pub(crate) title: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) servers: Vec<String>,
    pub(crate) schemes: Vec<Scheme>,
    /// The sign-in every operation needs unless it says otherwise.
    pub(crate) security: Vec<String>,
    pub(crate) operations: Vec<Operation>,
    /// `components.schemas` (Swagger 2.0's `definitions`), by name.
    pub(crate) schemas: Vec<(String, Value)>,
    /// How it was found from what was given, for a URL.
    pub(crate) found: Option<find::Found>,
}

pub(crate) struct Scheme {
    pub(crate) name: String,
    /// `bearer`, `basic`, `apiKey header X-Api-Key`, `oauth2`, `openIdConnect`.
    pub(crate) kind: String,
    pub(crate) detail: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Operation {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) operation_id: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) deprecated: bool,
    pub(crate) parameters: Vec<Param>,
    pub(crate) body: Option<RequestBody>,
    pub(crate) responses: Vec<ResponseDoc>,
    /// `None`: the spec's own; empty: none at all.
    pub(crate) security: Option<Vec<String>>,
}

#[derive(Clone)]
pub(crate) struct Param {
    pub(crate) name: String,
    /// `path`, `query`, `header`, `cookie`.
    pub(crate) location: String,
    pub(crate) required: bool,
    pub(crate) schema: Value,
    pub(crate) description: Option<String>,
    pub(crate) deprecated: bool,
}

#[derive(Clone)]
pub(crate) struct RequestBody {
    pub(crate) required: bool,
    /// Media type and schema, in the spec's order.
    pub(crate) content: Vec<(String, Value)>,
}

#[derive(Clone)]
pub(crate) struct ResponseDoc {
    pub(crate) status: String,
    pub(crate) description: Option<String>,
    pub(crate) content: Vec<(String, Value)>,
}

impl Operation {
    /// `POST /orders/{}`: the path with its parameters' names left out, so
    /// `/orders/{id}` and `/orders/{orderId}` are one operation.
    pub(crate) fn key(&self) -> String {
        format!("{} {}", self.method, template(&self.path))
    }

    /// What `operation get` takes: `SPEC#METHOD PATH`.
    pub(crate) fn id(&self, spec: &Spec) -> String {
        format!("{}#{} {}", spec.id, self.method, self.path)
    }

    /// The sign-in it needs, the spec's own unless it says.
    pub(crate) fn needs<'a>(&'a self, spec: &'a Spec) -> &'a [String] {
        self.security.as_deref().unwrap_or(&spec.security)
    }

    /// The body's schema for JSON, else its first.
    pub(crate) fn body_schema(&self) -> Option<&Value> {
        let body = self.body.as_ref()?;
        body.content
            .iter()
            .find(|(media, _)| media.contains("json"))
            .or_else(|| body.content.first())
            .map(|(_, schema)| schema)
    }
}

impl Spec {
    /// The components `op` uses, through the components they use.
    pub(crate) fn uses(&self, op: &Operation) -> Vec<String> {
        let mut found = Vec::new();
        let mut pending = Vec::new();
        for param in &op.parameters {
            refs(&param.schema, &mut pending);
        }
        for (_, schema) in op.body.iter().flat_map(|body| &body.content) {
            refs(schema, &mut pending);
        }
        for (_, schema) in op.responses.iter().flat_map(|doc| &doc.content) {
            refs(schema, &mut pending);
        }
        while let Some(name) = pending.pop() {
            if found.contains(&name) {
                continue;
            }
            if let Some((_, schema)) = self.schemas.iter().find(|(held, _)| *held == name) {
                refs(schema, &mut pending);
            }
            found.push(name);
        }
        found
    }
}

/// The components a schema's `$ref`s name.
fn refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(name) = Spec::ref_name(value).filter(|_| map.contains_key("$ref")) {
                out.push(name);
            }
            for item in map.values() {
                refs(item, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| refs(item, out)),
        _ => {}
    }
}

/// A name with its case and separators gone, so `customerId`,
/// `customer_id` and `CustomerID` are one.
pub(crate) fn squash(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A path with `{name}` written `{}`.
pub(crate) fn template(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut inside = false;
    for c in path.chars() {
        match c {
            '{' => {
                inside = true;
                out.push_str("{}");
            }
            '}' => inside = false,
            _ if inside => {}
            c => out.push(c),
        }
    }
    out
}

impl Spec {
    /// Reads a parsed document. `id` is where it came from; a URL resolves
    /// relative servers, and is the origin a missing one stands for.
    pub(crate) fn read(id: &str, raw: Value) -> Result<Self> {
        let (format, two) = match (raw["openapi"].as_str(), raw["swagger"].as_str()) {
            (Some(version), _) => (format!("OpenAPI {version}"), false),
            (None, Some(version)) => (format!("Swagger {version}"), true),
            _ => {
                return Err(Failure::usage(format!(
                    "{id} is not an OpenAPI or Swagger document (no top-level openapi or swagger)"
                ))
                .into());
            }
        };
        let info = &raw["info"];
        let text = |value: &Value| value.as_str().map(str::to_owned).filter(|s| !s.is_empty());
        let servers = if two {
            read::servers_two(id, &raw)
        } else {
            read::servers_three(id, &raw)
        };
        let schemes = if two {
            read::schemes_of(&raw["securityDefinitions"])
        } else {
            read::schemes_of(&raw["components"]["securitySchemes"])
        };
        let schemas_at = if two {
            &raw["definitions"]
        } else {
            &raw["components"]["schemas"]
        };
        let schemas = schemas_at
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(name, schema)| (name.clone(), schema.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut spec = Self {
            id: id.to_owned(),
            format,
            title: text(&info["title"]),
            version: text(&info["version"])
                .or_else(|| info["version"].as_f64().map(|v| v.to_string())),
            description: text(&info["description"]),
            servers,
            schemes,
            security: read::requirement(&raw["security"]).unwrap_or_default(),
            operations: Vec::new(),
            schemas,
            found: None,
            raw: Value::Null,
        };
        spec.operations = read::operations(&raw, two);
        spec.raw = raw;
        Ok(spec)
    }

    /// The URL an operation answers at: the first server, or the spec's own
    /// origin when the server is missing or names localhost and the spec
    /// came from elsewhere.
    pub(crate) fn base(&self) -> Option<String> {
        let from_url = self.id.starts_with("http://") || self.id.starts_with("https://");
        let spec_origin = from_url.then(|| url::origin(&self.id));
        match self.servers.first() {
            Some(server) => {
                let local = |u: &str| {
                    let o = url::origin(u);
                    o.contains("://localhost")
                        || o.contains("://127.0.0.1")
                        || o.contains("://0.0.0.0")
                };
                match &spec_origin {
                    Some(origin) if local(server) && !local(origin) => Some(format!(
                        "{origin}{}",
                        url::path(server).trim_end_matches('/')
                    )),
                    _ => Some(server.trim_end_matches('/').to_owned()),
                }
            }
            None => spec_origin,
        }
    }

    /// The schema a `$ref` points at, followed through further refs; the
    /// value itself when it is no ref, `None` for one this document does
    /// not hold.
    pub(crate) fn deref<'a>(&'a self, value: &'a Value) -> Option<&'a Value> {
        resolve(&self.raw, value)
    }

    /// The component a `$ref` names: `Order` for `#/components/schemas/Order`.
    pub(crate) fn ref_name(value: &Value) -> Option<String> {
        let target = value["$ref"].as_str()?;
        target.rsplit('/').next().map(unescape)
    }
}

fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// `value` with its `$ref`s followed in `root`, at most ten deep.
pub(crate) fn resolve<'a>(root: &'a Value, value: &'a Value) -> Option<&'a Value> {
    let mut at = value;
    for _ in 0..10 {
        let Some(target) = at.get("$ref").and_then(Value::as_str) else {
            return Some(at);
        };
        let pointer = target.strip_prefix('#')?;
        at = root.pointer(pointer)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn swagger_two_reads_as_openapi_three() {
        let raw = json!({
            "swagger": "2.0",
            "info": {"title": "Orders", "version": "1.0"},
            "host": "", "basePath": "", "schemes": [],
            "paths": {"/orders": {"post": {
                "consumes": ["application/json"],
                "parameters": [
                    {"in": "body", "name": "order", "required": true, "schema": {"$ref": "#/definitions/main.OrderIn"}},
                    {"in": "query", "name": "dry", "type": "boolean"}
                ],
                "responses": {"201": {"description": "made", "schema": {"$ref": "#/definitions/main.Order"}}}
            }}},
            "definitions": {"main.Order": {"type": "object"}, "main.OrderIn": {"type": "object"}}
        });
        let spec = Spec::read("http://127.0.0.1:58086/swagger/doc.json", raw).unwrap();
        assert_eq!(spec.format, "Swagger 2.0");
        assert_eq!(spec.base().as_deref(), Some("http://127.0.0.1:58086"));
        let op = &spec.operations[0];
        assert_eq!(op.key(), "POST /orders");
        assert_eq!(op.parameters.len(), 1);
        assert_eq!(op.parameters[0].schema, json!({"type": "boolean"}));
        let body = op.body.as_ref().unwrap();
        assert!(body.required);
        assert_eq!(body.content[0].0, "application/json");
        assert_eq!(
            op.responses[0].content[0].1,
            json!({"$ref": "#/definitions/main.Order"})
        );
        assert_eq!(spec.schemas.len(), 2);
    }

    #[test]
    fn a_localhost_server_stands_for_the_origin_the_spec_came_from() {
        let raw = json!({"openapi": "3.0.4", "info": {"title": "t", "version": "1"},
            "servers": [{"url": "http://localhost:5000/api"}], "paths": {}});
        let spec = Spec::read("https://api.contoso.example/openapi/v1.json", raw).unwrap();
        assert_eq!(
            spec.base().as_deref(),
            Some("https://api.contoso.example/api")
        );
        let relative =
            json!({"openapi": "3.1.0", "info": {}, "servers": [{"url": "/v2"}], "paths": {}});
        let spec = Spec::read("https://api.contoso.example/openapi.json", relative).unwrap();
        assert_eq!(
            spec.base().as_deref(),
            Some("https://api.contoso.example/v2")
        );
    }

    #[test]
    fn path_parameters_are_shared_and_an_operations_own_win() {
        let raw = json!({"openapi": "3.1.0", "info": {},
        "security": [{"bearer": []}],
        "paths": {"/orders/{order_id}": {
            "parameters": [{"in": "path", "name": "order_id", "schema": {"type": "integer"}}],
            "get": {"operationId": "get_order", "security": [], "parameters": [{"in": "path", "name": "order_id", "required": true, "schema": {"type": "string"}}], "responses": {}}
        }}});
        let spec = Spec::read("o.json", raw).unwrap();
        let op = &spec.operations[0];
        assert_eq!(op.parameters.len(), 1);
        assert_eq!(op.parameters[0].schema["type"], "string");
        assert_eq!(op.key(), "GET /orders/{}");
        assert!(op.needs(&spec).is_empty());
        assert_eq!(spec.security, ["bearer"]);
    }
}
