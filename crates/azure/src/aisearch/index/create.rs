//! `aisearch index create`: `PUT /indexes/{name}` with `If-None-Match: *`,
//! so it never replaces an index that is already there.

use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;

use crate::aisearch::index::{IndexDetail, definition, detail, named};
use crate::aisearch::{Role, Search, one, segment};
use crate::client::refused_with;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct IndexCreateArgs {
    /// The new index's name, or SERVICE/INDEX
    index: String,
    /// The service to create it on; needed when there is more than one
    #[arg(long)]
    service: Option<String>,
    /// The index definition: JSON, or - to read it from stdin
    #[arg(long, allow_hyphen_values = true)]
    definition: Option<String>,
    /// The index definition from a JSON file
    #[arg(long)]
    definition_file: Option<PathBuf>,
}

fn index_create(ctx: &Ctx, args: IndexCreateArgs) -> Result<IndexDetail> {
    let azure = Azure::load(ctx)?;
    let (service, name) = named(&args.index, args.service.as_deref())?;
    let mut body = definition(
        ctx,
        &name,
        args.definition.as_deref(),
        args.definition_file.as_deref(),
    )?;
    if let Some(object) = body.as_object_mut() {
        object.remove("@odata.etag");
    }
    let search = Search::new(ctx, one(ctx, &azure, service.as_deref())?)?;
    let created = search
        .change(
            Effect::Write,
            Method::Put,
            &format!("/indexes/{}", segment(&name)),
            Some(body),
            &[
                ("If-None-Match", "*".to_owned()),
                ("Prefer", "return=representation".to_owned()),
            ],
            Role::Manage,
        )
        .map_err(|error| {
            match refused_with(&error) {
            Some(412) => Failure::conflict(format!(
                "index {name} is already on {}",
                search.name()
            ))
            .hint(format!(
                "change it with `agent-cli aisearch index update {}/{name} --definition-file FILE`",
                search.name()
            ))
            .into(),
            _ => error,
        }
        })?
        .json()?;
    Ok(detail(search.name(), &created, None))
}

command! {
    pub INDEX_CREATE = ["aisearch", "index", "create"], Write,
    "Create an AI Search index from a JSON definition (never replaces one)",
    keywords: ["new", "schema", "definition", "add", "fields"],
    example: "aisearch index create srch-contoso-dev/orders-v2 --definition-file orders.json",
    run: index_create,
}

#[cfg(test)]
mod tests {
    use crate::AISEARCH;
    use crate::testing::{self, azure};
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, assert_dry_run, run};

    #[test]
    fn create_puts_the_definition_only_if_none_is_there() {
        let plans = assert_dry_run(
            &[AISEARCH],
            &[
                "aisearch",
                "index",
                "create",
                "srch-contoso-prod/orders-v2",
                "--definition",
                r#"{"fields": [{"name": "id", "type": "Edm.String", "key": true}]}"#,
            ],
            vec![testing::inventory(vec![testing::search_service(
                "srch-contoso-prod",
                "aadOrApiKey",
            )])],
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(
            plans[0]["url"],
            "https://srch-contoso-prod.search.windows.net/indexes/orders-v2?api-version=2026-04-01"
        );
        assert_eq!(plans[0]["headers"]["If-None-Match"], "*");
        assert_eq!(plans[0]["body"]["name"], "orders-v2");

        let transport = FakeTransport::answering([
            testing::inventory(vec![testing::search_service(
                "srch-contoso-prod",
                "aadOrApiKey",
            )]),
            Answer::status(
                412,
                r#"{"error":{"code":"","message":"The precondition given in one of the request headers evaluated to false."}}"#,
            ),
        ]);
        let outcome = run(
            &[AISEARCH],
            &["aisearch", "index", "create", "orders", "--definition", "-"],
            Setup::fake(transport)
                .with_config(testing::SERIAL)
                .with_stdin(r#"{"name": "orders", "fields": []}"#),
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("index orders is already on srch-contoso-prod"),
            "{}",
            outcome.stderr
        );
        let (outcome, _) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "index",
                "create",
                "orders",
                "--definition",
                r#"{"name": "other"}"#,
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
