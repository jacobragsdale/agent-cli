//! `aisearch index update`: `PUT /indexes/{name}` with `If-Match`, so it
//! never creates one and never overwrites a change it did not see.

use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;

use crate::aisearch::index::{IndexDetail, definition, detail, named};
use crate::aisearch::{Role, holder, segment};
use crate::client::text;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct IndexUpdateArgs {
    /// The index: SERVICE/INDEX from index list, or a bare name
    index: String,
    /// The service that holds it; needed when more than one does
    #[arg(long)]
    service: Option<String>,
    /// The whole new definition: JSON, or - to read it from stdin
    #[arg(long, allow_hyphen_values = true)]
    definition: Option<String>,
    /// The new definition from a JSON file, as index get --full --output saves it
    #[arg(long)]
    definition_file: Option<PathBuf>,
    /// Change it only if its etag is still this (default: the definition's @odata.etag)
    #[arg(long)]
    if_etag: Option<String>,
    /// Allow adding analyzers, which takes the index offline for a few seconds
    #[arg(long)]
    allow_downtime: bool,
}

fn index_update(ctx: &Ctx, args: IndexUpdateArgs) -> Result<IndexDetail> {
    let azure = Azure::load(ctx)?;
    let (service, name) = named(&args.index, args.service.as_deref())?;
    let body = definition(
        ctx,
        &name,
        args.definition.as_deref(),
        args.definition_file.as_deref(),
    )?;
    // `*` still refuses to create one: an update needs the index there.
    let etag = args
        .if_etag
        .clone()
        .or_else(|| text(&body["@odata.etag"]))
        .unwrap_or_else(|| "*".to_owned());
    let search = holder(ctx, &azure, "indexes", &name, service.as_deref())?;
    let mut path = format!("/indexes/{}", segment(&name));
    if args.allow_downtime {
        path.push_str("?allowIndexDowntime=true");
    }
    let id = format!("{}/{name}", search.name());
    let updated = search
        .change(
            Effect::Write,
            Method::Put,
            &path,
            Some(body),
            &[
                ("If-Match", etag),
                ("Prefer", "return=representation".to_owned()),
            ],
            Role::Manage,
        )
        .map_err(|error| refused(error, &id))?
        .json()?;
    Ok(detail(search.name(), &updated, None))
}

/// The two refusals an update meets, with what to do instead.
fn refused(error: anyhow::Error, id: &str) -> anyhow::Error {
    let said = format!("{error:#}");
    if said.contains("CannotChangeExistingField") || said.contains("cannot be changed") {
        return Failure::usage(said)
            .hint("a field's type, attributes or dimensions cannot change in place: create a new index (agent-cli aisearch index create NAME --definition-file FILE), load it, then point an alias at it")
            .into();
    }
    if crate::client::refused_with(&error) == Some(412) {
        return Failure::conflict(format!("{id} changed since its definition was read"))
            .hint(format!(
                "read it again with `agent-cli aisearch index get {id} --full --output FILE`, then redo the change"
            ))
            .into();
    }
    error
}

command! {
    pub INDEX_UPDATE = ["aisearch", "index", "update"], Write,
    "Change an AI Search index's definition (fields added, semantic, scoring, CORS)",
    keywords: ["modify", "edit", "schema", "add", "field", "alter", "etag"],
    example: "aisearch index update srch-contoso-prod/orders --definition-file orders.json",
    run: index_update,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, assert_dry_run};

    use crate::AISEARCH;
    use crate::testing::{self, azure};

    #[test]
    fn update_sends_the_definitions_etag_and_names_the_way_round_a_refused_change() {
        let one = || {
            testing::inventory(vec![testing::search_service(
                "srch-contoso-prod",
                "aadOrApiKey",
            )])
        };
        let definition =
            r#"{"name": "orders", "@odata.etag": "\"0x8DCE0A1B2C3D4E5\"", "fields": []}"#;
        let plans = assert_dry_run(
            &[AISEARCH],
            &[
                "aisearch",
                "index",
                "update",
                "orders",
                "--definition",
                definition,
                "--allow-downtime",
            ],
            vec![one()],
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(plans[0]["headers"]["If-Match"], "\"0x8DCE0A1B2C3D4E5\"");
        assert_eq!(
            plans[0]["url"],
            "https://srch-contoso-prod.search.windows.net/indexes/orders?allowIndexDowntime=true&api-version=2026-04-01"
        );
        let plans = assert_dry_run(
            &[AISEARCH],
            &[
                "aisearch",
                "index",
                "update",
                "orders",
                "--definition",
                r#"{"fields": []}"#,
            ],
            vec![one()],
        );
        assert_eq!(plans[0]["headers"]["If-Match"], "*");

        let (outcome, _) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "index",
                "update",
                "orders",
                "--definition",
                definition,
            ],
            vec![
                one(),
                Answer::status(
                    400,
                    r#"{"error":{"code":"OperationNotAllowed","message":"Existing field 'total' cannot be changed.","details":[{"code":"CannotChangeExistingField","message":"Existing field 'total' cannot be changed."}]}}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("point an alias at it"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "index",
                "update",
                "orders",
                "--definition",
                definition,
            ],
            vec![
                one(),
                Answer::status(
                    412,
                    r#"{"error":{"code":"","message":"The precondition given in one of the request headers evaluated to false."}}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
    }
}
