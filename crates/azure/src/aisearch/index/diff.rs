//! `aisearch index diff`: one index on two services (dev's and prod's), or
//! two indexes, by schema as `index get` summarizes it: fields by name and
//! their attributes, analyzers and vector dimensions, semantic configs,
//! vector profiles and vectorizers, scoring profiles and suggesters. Counts,
//! sizes and etags are each index's own.

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Side, command, diff};
use anyhow::Result;

use crate::aisearch::index::{detail, named};
use crate::aisearch::{Role, holder, segment};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct IndexDiffArgs {
    /// One side: SERVICE/INDEX from index list, its URL or portal link
    left: String,
    /// The other side, as left
    right: String,
    #[command(flatten)]
    diff: DiffArgs,
}

const RULES: Rules = Rules {
    keys: &[
        ("fields", "name"),
        ("fields.*.fields", "name"),
        ("semantic.configs", "name"),
        ("vector.profiles", "name"),
        ("vector.vectorizers", "name"),
        ("suggesters", "name"),
    ],
    noise: &[
        "id",
        "service",
        "name",
        "documents",
        "storage",
        "vector_storage",
        "etag",
    ],
    ..Rules::PLAIN
};

fn index_diff(ctx: &Ctx, args: IndexDiffArgs) -> Result<Diff> {
    let azure = Azure::load(ctx)?;
    let side = |raw: &str| -> Result<Side> {
        let (service, name) = named(raw, None)?;
        let search = holder(ctx, &azure, "indexes", &name, service.as_deref())?;
        let definition = search.get(&format!("/indexes/{}", segment(&name)), Role::Definitions)?;
        let row = serde_json::to_value(detail(search.name(), &definition, None))?;
        Ok(Side::new(format!("{}/{name}", search.name()), row))
    };
    let (left, right) = (side(&args.left)?, side(&args.right)?);
    diff(ctx, &args.diff, &RULES, left, right)
}

command! {
    pub INDEX_DIFF = ["aisearch", "index", "diff"], Read,
    "Compare two AI Search indexes: fields, analyzers, semantic configs, vectors",
    keywords: ["staging", "schema", "fields", "filterable", "drift"],
    example: "aisearch index diff srch-contoso-dev/orders srch-contoso-prod/orders",
    run: index_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, admin_keys, azure, search_service};

    fn index(filterable: bool) -> Answer {
        Answer::json(
            &json!({"name": "orders", "@odata.etag": "\"0x1\"", "fields": [
                {"name": "id", "type": "Edm.String", "key": true, "searchable": false, "filterable": true},
                {"name": "status", "type": "Edm.String", "searchable": true, "filterable": filterable},
            ]}),
        )
    }

    #[test]
    fn one_index_on_two_services_differs_by_a_fields_attributes() {
        let (outcome, _) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "index",
                "diff",
                "srch-contoso-dev/orders",
                "srch-contoso-prod/orders",
            ],
            vec![
                testing::inventory(vec![
                    search_service("srch-contoso-dev", "apiKeyOnly"),
                    search_service("srch-contoso-prod", "apiKeyOnly"),
                ]),
                admin_keys(),
                index(false),
                testing::inventory(vec![
                    search_service("srch-contoso-dev", "apiKeyOnly"),
                    search_service("srch-contoso-prod", "apiKeyOnly"),
                ]),
                admin_keys(),
                index(true),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["as"], json!(["dev=prod"]));
        assert_eq!(
            got["changes"][0],
            json!({"path": "fields.status.attrs", "left": ["searchable"],
                "right": ["searchable", "filterable"]}),
            "{got}"
        );
    }
}
