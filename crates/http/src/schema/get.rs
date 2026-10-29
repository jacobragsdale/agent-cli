use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::call::Fetch;
use crate::openapi::fields::{Field, Node, flatten};
use crate::openapi::load;

#[derive(clap::Args)]
pub struct SchemaGetArgs {
    /// SPEC#Name, as schema list prints it
    schema: String,
    /// The spec, when the schema is given by name alone
    #[arg(long, value_name = "SPEC")]
    spec: Option<String>,
    #[command(flatten)]
    fetch: Fetch,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SchemaDetail {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    description: Option<String>,
    fields: Vec<Field>,
    /// The operations using it, through other schemas too: METHOD PATH.
    used_by: Vec<String>,
}

fn schema_get(ctx: &Ctx, args: SchemaGetArgs) -> Result<SchemaDetail> {
    let (spec_ref, wanted) = load::split(&args.schema, args.spec.as_deref(), "Name")?;
    let prepared = args.fetch.prepare(ctx)?;
    let spec = load::spec(ctx, &prepared, spec_ref)?;
    let Some((name, schema)) = spec
        .schemas
        .iter()
        .find(|(name, _)| name == wanted)
        .or_else(|| {
            spec.schemas
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
        })
    else {
        return Err(
            Failure::not_found(format!("{} has no schema {wanted:?}", spec.id))
                .hint(format!(
                    "agent-cli http schema list {}  lists them",
                    spec.id
                ))
                .into(),
        );
    };
    let node = Node::of(&spec, schema);
    Ok(SchemaDetail {
        id: format!("{}#{name}", spec.id),
        name: name.clone(),
        kind: node.kind.clone(),
        description: node.description.clone(),
        fields: flatten(&spec, schema),
        used_by: spec
            .operations
            .iter()
            .filter(|op| spec.uses(op).contains(name))
            .map(|op| format!("{} {}", op.method, op.path))
            .collect(),
    })
}

command! {
    pub SCHEMA_GET = ["http", "schema", "get"], Read,
    "Show a spec's schema (model) as field paths: types, required, enums, users",
    keywords: ["openapi", "swagger", "model", "dto", "component", "definition", "fields"],
    example: "http schema get 'https://api.contoso.example/openapi.json#Order'",
    run: schema_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};

    use crate::DOMAIN;
    use crate::testing::spec_v1;

    #[test]
    fn a_schema_shows_its_fields_and_who_uses_it() {
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "schema",
                "get",
                "Order",
                "--spec",
                "https://api.contoso.example/openapi.json",
            ],
            Setup::fake(FakeTransport::answering([Answer::json(&spec_v1())])),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let detail = outcome.json();
        assert_eq!(
            detail["id"],
            "https://api.contoso.example/openapi.json#Order"
        );
        let paths: Vec<&str> = detail["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "id",
                "status",
                "customer_id",
                "lines",
                "lines[].sku",
                "lines[].qty",
                "note"
            ]
        );
        assert_eq!(detail["used_by"].as_array().unwrap().len(), 3);
    }
}
