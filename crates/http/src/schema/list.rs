use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::call::Fetch;
use crate::openapi::fields::{Node, cut, flatten};
use crate::openapi::{load, squash};

#[derive(clap::Args)]
pub struct SchemaListArgs {
    /// The spec: its URL, its docs page, the API's base URL, or a file
    spec: String,
    /// Words its name or description holds
    words: Vec<String>,
    /// Only schemas with a field so named: email
    #[arg(long)]
    field: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[command(flatten)]
    fetch: Fetch,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SchemaRow {
    /// What `http schema get` takes: SPEC#Name.
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    fields: usize,
    /// How many operations use it, through other schemas too.
    used_by: usize,
    description: Option<String>,
}

fn schema_list(ctx: &Ctx, args: SchemaListArgs) -> Result<Vec<SchemaRow>> {
    let prepared = args.fetch.prepare(ctx)?;
    let spec = load::spec(ctx, &prepared, &args.spec)?;
    let words: Vec<String> = args
        .words
        .iter()
        .flat_map(|w| w.split_whitespace())
        .map(str::to_lowercase)
        .collect();
    let uses: Vec<Vec<String>> = spec.operations.iter().map(|op| spec.uses(op)).collect();
    let mut rows = Vec::new();
    for (name, schema) in &spec.schemas {
        let node = Node::of(&spec, schema);
        let fields = flatten(&spec, schema);
        let text = format!("{name} {}", node.description.as_deref().unwrap_or("")).to_lowercase();
        if !words.iter().all(|word| text.contains(word.as_str())) {
            continue;
        }
        if let Some(wanted) = &args.field {
            let wanted = squash(wanted);
            if !fields
                .iter()
                .any(|f| squash(f.path.rsplit(['.', ']', '}']).next().unwrap_or(&f.path)) == wanted)
            {
                continue;
            }
        }
        rows.push(SchemaRow {
            id: format!("{}#{name}", spec.id),
            name: name.clone(),
            kind: node.kind.clone(),
            fields: fields.len(),
            used_by: uses.iter().filter(|used| used.contains(name)).count(),
            description: node.description.as_deref().map(|d| cut(d, 160)),
        });
    }
    let total = rows.len();
    rows.truncate(args.limit);
    if total > rows.len() {
        ctx.note(format!("[{} of {total}; --limit N]", rows.len()));
    }
    Ok(rows)
}

command! {
    pub SCHEMA_LIST = ["http", "schema", "list"], Read,
    "List a spec's schemas (models, DTOs), or those with a field so named",
    keywords: ["openapi", "swagger", "models", "dto", "components", "definitions", "field"],
    example: "http schema list https://api.contoso.example --field email --fields id,fields,used_by",
    run: schema_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};

    use crate::DOMAIN;
    use crate::testing::spec_v1;

    #[test]
    fn schemas_list_their_fields_and_users_and_filter_by_a_field() {
        let run_with = |argv: &[&str]| {
            let outcome = run(
                &[DOMAIN],
                argv,
                Setup::fake(FakeTransport::answering([Answer::json(&spec_v1())])),
            );
            assert_eq!(outcome.code, 0, "{outcome:?}");
            outcome.json()
        };
        let all = run_with(&[
            "http",
            "schema",
            "list",
            "https://api.contoso.example/openapi.json",
        ]);
        let order = all
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "Order")
            .unwrap();
        assert_eq!(order["used_by"], 3);
        let with_sku = run_with(&[
            "http",
            "schema",
            "list",
            "https://api.contoso.example/openapi.json",
            "--field",
            "sku",
        ]);
        let names: Vec<&str> = with_sku
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Order", "OrderIn", "Line"]);
    }
}
