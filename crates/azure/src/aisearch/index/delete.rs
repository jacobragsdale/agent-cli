//! `aisearch index delete`: `DELETE /indexes/{name}`, with its documents.

use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::aisearch::index::named;
use crate::aisearch::{Role, holder, segment};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct IndexDeleteArgs {
    /// The index: SERVICE/INDEX from index list, or a bare name (not an alias)
    index: String,
    /// The service that holds it; needed when more than one does
    #[arg(long)]
    service: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Deleted {
    id: String,
    deleted: bool,
}

fn index_delete(ctx: &Ctx, args: IndexDeleteArgs) -> Result<Deleted> {
    let azure = Azure::load(ctx)?;
    let (service, name) = named(&args.index, args.service.as_deref())?;
    let search = holder(ctx, &azure, "indexes", &name, service.as_deref())?;
    // The service refuses (400) an index an alias points at, in its own words.
    search.change(
        Effect::Destructive,
        Method::Delete,
        &format!("/indexes/{}", segment(&name)),
        None,
        &[],
        Role::Manage,
    )?;
    Ok(Deleted {
        id: format!("{}/{name}", search.name()),
        deleted: true,
    })
}

command! {
    pub INDEX_DELETE = ["aisearch", "index", "delete"], Destructive,
    "Delete an AI Search index and every document in it",
    keywords: ["drop", "remove", "destroy"],
    example: "aisearch index delete srch-contoso-dev/orders-v2 --yes",
    run: index_delete,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, assert_dry_run};
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, admin_keys, azure};

    #[test]
    fn delete_needs_yes_and_sends_the_key_to_a_key_only_service() {
        let one = || {
            testing::inventory(vec![testing::search_service(
                "srch-contoso-dev",
                "apiKeyOnly",
            )])
        };
        let plans = assert_dry_run(
            &[AISEARCH],
            &["aisearch", "index", "delete", "srch-contoso-dev/orders-v2"],
            vec![one(), admin_keys()],
        );
        assert_eq!(plans[0]["method"], "DELETE");
        assert_eq!(plans[0]["headers"]["api-key"], "***", "{plans:?}");

        let (outcome, transport) = azure(
            &[AISEARCH],
            &["aisearch", "index", "delete", "orders-v2"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());

        let (outcome, _) = azure(
            &[AISEARCH],
            &["aisearch", "index", "delete", "orders-v2", "--yes"],
            vec![one(), admin_keys(), Answer::status(204, "")],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "srch-contoso-dev/orders-v2", "deleted": true})
        );
    }
}
