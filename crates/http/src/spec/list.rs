use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::call::Fetch;
use crate::openapi::{Spec, find, load};

#[derive(clap::Args)]
pub struct SpecListArgs {
    /// The API's base URL, its docs page, or one of its specs
    url: String,
    /// Most specs to read
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[command(flatten)]
    fetch: Fetch,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SpecListRow {
    /// What the operation, schema and diff verbs take as SPEC.
    id: String,
    /// The name its docs page gives it (a version, a group).
    name: String,
    format: Option<String>,
    title: Option<String>,
    version: Option<String>,
    operations: Option<usize>,
    /// Why it could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<String>,
}

fn spec_list(ctx: &Ctx, args: SpecListArgs) -> Result<Vec<SpecListRow>> {
    let prepared = args.fetch.prepare(ctx)?;
    let listed = find::all(ctx, &prepared, &args.url)?;
    let total = listed.len();
    let rows = listed
        .into_iter()
        .take(args.limit)
        .map(|entry| {
            let read = prepared
                .send(
                    ctx,
                    agent_cli_core::Method::Get,
                    &entry.url,
                    agent_cli_core::Body::None,
                    true,
                )
                .and_then(|exchange| {
                    if exchange.response.status >= 300 {
                        anyhow::bail!("answered {}", exchange.response.status);
                    }
                    Spec::read(&entry.url, load::parse(&exchange.response.body)?)
                });
            match read {
                Ok(spec) => SpecListRow {
                    id: entry.url,
                    name: entry.name,
                    format: Some(spec.format),
                    title: spec.title,
                    version: spec.version,
                    operations: Some(spec.operations.len()),
                    problem: None,
                },
                Err(error) => SpecListRow {
                    id: entry.url,
                    name: entry.name,
                    format: None,
                    title: None,
                    version: None,
                    operations: None,
                    problem: Some(format!("{error:#}")),
                },
            }
        })
        .collect::<Vec<_>>();
    if total > rows.len() {
        ctx.note(format!("[{} of {total}; --limit N]", rows.len()));
    }
    Ok(rows)
}

command! {
    pub SPEC_LIST = ["http", "spec", "list"], Read,
    "List the OpenAPI specs an API publishes: its versions and groups",
    keywords: ["openapi", "swagger", "groups", "springdoc", "v1", "v2", "specs"],
    example: "http spec list https://api.contoso.example --fields id,name,version,operations",
    run: spec_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};

    use crate::DOMAIN;
    use crate::testing::spec_v1;

    #[test]
    fn springdocs_groups_are_listed_from_its_swagger_config() {
        let config = r#"{"configUrl":"/v3/api-docs/swagger-config","urls":[{"url":"/v3/api-docs/admin","name":"admin"},{"url":"/v3/api-docs/orders","name":"orders"}]}"#;
        let initializer = "window.ui = SwaggerUIBundle({ url: \"https://petstore.swagger.io/v2/swagger.json\", \"configUrl\" : \"/v3/api-docs/swagger-config\" });";
        let transport = FakeTransport::answering([
            Answer::status(404, "{}"),
            Answer::status(404, "{}"),
            Answer::status(404, "{}"),
            Answer::status(404, "{}"),
            Answer::json(&spec_v1()),
            Answer::ok("<script src=\"./swagger-initializer.js\"></script>")
                .with_header("Content-Type", "text/html"),
            Answer::ok("<script src=\"./swagger-initializer.js\"></script>")
                .with_header("Content-Type", "text/html"),
            Answer::ok(initializer),
            Answer::ok(config),
            Answer::json(&spec_v1()),
            Answer::json(&spec_v1()),
            Answer::json(&spec_v1()),
        ]);
        let outcome = run(
            &[DOMAIN],
            &["http", "spec", "list", "https://api.contoso.example"],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let names: Vec<String> = outcome
            .json()
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            ["default", "admin", "orders"],
            "the default is springdoc's all"
        );
    }
}
