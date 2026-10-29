use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::call::Fetch;
use crate::openapi::example::example;
use crate::openapi::fields::{Field, cut, flatten};
use crate::openapi::load;

#[derive(clap::Args)]
pub struct OperationGetArgs {
    /// SPEC#METHOD PATH (or SPEC#operationId), as operation list prints it
    operation: String,
    /// The spec, when the operation is given without it
    #[arg(long, value_name = "SPEC")]
    spec: Option<String>,
    #[command(flatten)]
    fetch: Fetch,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OperationDetail {
    id: String,
    method: String,
    path: String,
    /// Where it answers: the spec's base and the path.
    url: Option<String>,
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    operation_id: Option<String>,
    tags: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    deprecated: bool,
    /// The sign-in it needs, or none.
    security: Vec<String>,
    parameters: Vec<ParamRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<BodyRow>,
    responses: Vec<ResponseRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ParamRow {
    name: String,
    #[serde(rename = "in")]
    location: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    required: bool,
    #[serde(flatten)]
    field: Field,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BodyRow {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    required: bool,
    content_types: Vec<String>,
    fields: Vec<Field>,
    /// A body to start from: its required fields, each of its type.
    example: Value,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ResponseRow {
    status: String,
    description: Option<String>,
    content_types: Vec<String>,
    fields: Vec<Field>,
}

fn operation_get(ctx: &Ctx, args: OperationGetArgs) -> Result<OperationDetail> {
    let (spec_ref, wanted) = load::split(&args.operation, args.spec.as_deref(), "METHOD PATH")?;
    let prepared = args.fetch.prepare(ctx)?;
    let spec = load::spec(ctx, &prepared, spec_ref)?;
    let op = load::operation(&spec, wanted)?;
    let needs = op.needs(&spec);
    let parameters = op
        .parameters
        .iter()
        .map(|param| {
            let mut field = flatten(&spec, &param.schema)
                .into_iter()
                .next()
                .unwrap_or_else(empty);
            field.path = String::new();
            field.required = false;
            if field.description.is_none() {
                field.description = param.description.as_deref().map(|d| cut(d, 160));
            }
            field.deprecated |= param.deprecated;
            ParamRow {
                name: param.name.clone(),
                location: param.location.clone(),
                required: param.required,
                field,
            }
        })
        .collect();
    let body = op.body.as_ref().map(|body| {
        let schema = op.body_schema();
        BodyRow {
            required: body.required,
            content_types: body
                .content
                .iter()
                .map(|(media, _)| media.clone())
                .collect(),
            fields: schema.map(|s| flatten(&spec, s)).unwrap_or_default(),
            example: schema.map_or(Value::Null, |s| example(&spec, s)),
        }
    });
    let responses = op
        .responses
        .iter()
        .map(|doc| {
            let schema = doc
                .content
                .iter()
                .find(|(media, _)| media.contains("json"))
                .or_else(|| doc.content.first());
            ResponseRow {
                status: doc.status.clone(),
                description: doc.description.as_deref().map(|d| cut(d, 160)),
                content_types: doc.content.iter().map(|(media, _)| media.clone()).collect(),
                fields: schema.map(|(_, s)| flatten(&spec, s)).unwrap_or_default(),
            }
        })
        .collect();
    Ok(OperationDetail {
        id: op.id(&spec),
        url: spec.base().map(|base| format!("{base}{}", op.path)),
        method: op.method.clone(),
        path: op.path.clone(),
        summary: op.summary.clone(),
        description: op.description.as_deref().map(|d| cut(d, 1000)),
        operation_id: op.operation_id.clone(),
        tags: op.tags.clone(),
        deprecated: op.deprecated,
        security: if needs.is_empty() {
            vec!["none".to_owned()]
        } else {
            needs.to_vec()
        },
        parameters,
        body,
        responses,
    })
}

fn empty() -> Field {
    Field {
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
    }
}

command! {
    pub OPERATION_GET = ["http", "operation", "get"], Read,
    "Show an operation (endpoint): its parameters, body fields, answers, an example",
    keywords: ["endpoint", "openapi", "swagger", "parameters", "body", "fields", "example", "schema"],
    example: "http operation get 'https://api.contoso.example/openapi.json#POST /orders'",
    run: operation_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};
    use serde_json::json;

    use crate::DOMAIN;
    use crate::testing::spec_v1;

    #[test]
    fn an_operation_shows_its_url_fields_and_an_example_body() {
        let transport = FakeTransport::answering([Answer::json(&spec_v1())]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "operation",
                "get",
                "https://api.contoso.example/openapi.json#POST /orders",
            ],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let detail = outcome.json();
        assert_eq!(detail["url"], "https://api.contoso.example/orders");
        assert_eq!(detail["security"], json!(["Bearer"]));
        let fields: Vec<&str> = detail["body"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            fields,
            ["customer_id", "lines", "lines[].sku", "lines[].qty", "note"]
        );
        assert_eq!(
            detail["body"]["example"],
            json!({"customer_id": 0, "lines": [{"sku": "string"}]})
        );
        assert_eq!(detail["responses"][0]["status"], "201");
    }

    #[test]
    fn an_operation_id_works_with_spec_and_a_missing_one_names_its_neighbours() {
        let transport = FakeTransport::answering([Answer::json(&spec_v1())]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "operation",
                "get",
                "get_order",
                "--spec",
                "https://api.contoso.example/openapi.json",
            ],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["path"], "/orders/{order_id}");
        let missing = run(
            &[DOMAIN],
            &[
                "http",
                "operation",
                "get",
                "https://api.contoso.example/openapi.json#PUT /orders/{id}",
            ],
            Setup::fake(FakeTransport::answering([Answer::json(&spec_v1())])),
        );
        assert_eq!(missing.code, 4, "{missing:?}");
        assert!(
            missing.stderr.contains("near it: GET /orders/{order_id}"),
            "{}",
            missing.stderr
        );
    }
}
