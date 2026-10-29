use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::call::Fetch;
use crate::openapi::find::Listed;
use crate::openapi::{fields::cut, load};

#[derive(clap::Args)]
pub struct SpecGetArgs {
    /// The spec: its URL, its docs page, the API's base URL, or a file
    spec: String,
    #[command(flatten)]
    fetch: Fetch,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SpecRow {
    /// What the operation, schema and diff verbs take as SPEC.
    id: String,
    /// spec (the URL given), docs page, probed (a framework's path) or file.
    found: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    framework: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    docs: Option<String>,
    /// Fetches finding it took.
    #[serde(skip_serializing_if = "Option::is_none")]
    fetches: Option<usize>,
    format: String,
    title: Option<String>,
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    servers: Vec<String>,
    /// Where its operations answer: the first server, or the spec's own
    /// origin for none or one on localhost.
    base: Option<String>,
    sign_in: Vec<SchemeRow>,
    /// The sign-in operations need unless they say otherwise.
    security: Vec<String>,
    operations: usize,
    schemas: usize,
    tags: Vec<TagRow>,
    /// Other specs its docs page lists.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    others: Vec<Listed>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SchemeRow {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TagRow {
    name: String,
    operations: usize,
}

fn spec_get(ctx: &Ctx, args: SpecGetArgs) -> Result<SpecRow> {
    let prepared = args.fetch.prepare(ctx)?;
    let spec = load::spec(ctx, &prepared, &args.spec)?;
    let mut tags: Vec<TagRow> = Vec::new();
    for op in &spec.operations {
        let names = if op.tags.is_empty() {
            vec!["(untagged)".to_owned()]
        } else {
            op.tags.clone()
        };
        for name in names {
            match tags.iter_mut().find(|tag| tag.name == name) {
                Some(tag) => tag.operations += 1,
                None => tags.push(TagRow {
                    name,
                    operations: 1,
                }),
            }
        }
    }
    let found = spec.found.clone();
    Ok(SpecRow {
        base: spec.base(),
        id: spec.id.clone(),
        found: found
            .as_ref()
            .map_or_else(|| "file".to_owned(), |f| f.how.clone()),
        framework: found.as_ref().and_then(|f| f.framework.clone()),
        docs: found.as_ref().and_then(|f| f.docs.clone()),
        fetches: found.as_ref().map(|f| f.fetches),
        others: found.map(|f| f.others).unwrap_or_default(),
        format: spec.format.clone(),
        title: spec.title.clone(),
        version: spec.version.clone(),
        description: spec.description.as_deref().map(|d| cut(d, 300)),
        servers: spec.servers.clone(),
        sign_in: spec
            .schemes
            .iter()
            .map(|scheme| SchemeRow {
                name: scheme.name.clone(),
                kind: scheme.kind.clone(),
                detail: scheme.detail.clone(),
            })
            .collect(),
        security: spec.security.clone(),
        operations: spec.operations.len(),
        schemas: spec.schemas.len(),
        tags,
    })
}

command! {
    pub SPEC_GET = ["http", "spec", "get"], Read,
    "Find an API's OpenAPI spec from its base URL or docs page, and summarise it",
    keywords: ["openapi", "swagger", "discover", "fastapi", "aspnet", "springdoc", "redoc", "scalar", "spec", "json", "where"],
    example: "http spec get https://api.contoso.example",
    run: spec_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};
    use serde_json::json;

    use crate::DOMAIN;
    use crate::testing::{SWASHBUCKLE_PAGE, spec_v1};

    #[test]
    fn a_base_url_finds_the_spec_at_its_frameworks_path() {
        let transport = FakeTransport::answering([
            Answer::status(404, ""),
            Answer::status(404, ""),
            Answer::json(&spec_v1()).with_header("Content-Type", "application/json"),
            Answer::ok(SWASHBUCKLE_PAGE).with_header("Content-Type", "text/html"),
        ]);
        let outcome = run(
            &[DOMAIN],
            &["http", "spec", "get", "https://api.contoso.example"],
            Setup::fake(transport.clone()),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = outcome.json();
        assert_eq!(
            row["id"],
            "https://api.contoso.example/swagger/v1/swagger.json"
        );
        assert_eq!(row["found"], "probed");
        assert_eq!(row["framework"], "ASP.NET Core (Swashbuckle)");
        assert_eq!(
            row["docs"],
            "https://api.contoso.example/swagger/index.html"
        );
        assert_eq!(row["operations"], 4);
        assert_eq!(
            row["sign_in"][0],
            json!({"name": "Bearer", "type": "bearer", "detail": "JWT"})
        );
        let urls: Vec<String> = transport.sent().iter().map(|s| s.url.clone()).collect();
        assert_eq!(
            urls,
            [
                "https://api.contoso.example",
                "https://api.contoso.example/openapi.json",
                "https://api.contoso.example/swagger/v1/swagger.json",
                "https://api.contoso.example/swagger/index.html",
            ]
        );
    }

    #[test]
    fn a_docs_page_leads_to_the_spec_it_shows() {
        let transport = FakeTransport::answering([
            Answer::ok(SWASHBUCKLE_PAGE).with_header("Content-Type", "text/html"),
            Answer::ok(
                r#"var configObject = JSON.parse('{"urls":[{"url":"v1/swagger.json","name":"Orders v1"},{"url":"v2/swagger.json","name":"Orders v2"}]}');"#,
            ),
            Answer::json(&spec_v1()),
        ]);
        let outcome = run(
            &[DOMAIN],
            &[
                "http",
                "spec",
                "get",
                "https://api.contoso.example/swagger/index.html",
            ],
            Setup::fake(transport),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = outcome.json();
        assert_eq!(row["found"], "docs page");
        assert_eq!(
            row["id"],
            "https://api.contoso.example/swagger/v1/swagger.json"
        );
        assert_eq!(
            row["others"],
            json!([{"name": "v2/swagger.json", "url": "https://api.contoso.example/swagger/v2/swagger.json"}])
        );
    }

    #[test]
    fn a_yaml_file_reads_as_a_spec() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("openapi.yaml");
        std::fs::write(&path, "openapi: 3.1.0\ninfo:\n  title: orders API\n  version: 1.0.0\npaths:\n  /orders:\n    get:\n      responses:\n        200:\n          description: OK\n").unwrap();
        let outcome = run(
            &[DOMAIN],
            &["http", "spec", "get", path.to_str().unwrap()],
            Setup::fake(FakeTransport::default()),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["found"], "file");
        assert_eq!(outcome.json()["format"], "OpenAPI 3.1.0");
        assert_eq!(outcome.json()["operations"], 1);
    }

    #[test]
    fn nothing_found_is_exit_four_with_what_to_give_instead() {
        let answers = (0..80).map(|_| Answer::status(404, ""));
        let outcome = run(
            &[DOMAIN],
            &["http", "spec", "get", "https://api.contoso.example"],
            Setup::fake(FakeTransport::answering(answers)),
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("no OpenAPI spec found"),
            "{}",
            outcome.stderr
        );
    }
}
