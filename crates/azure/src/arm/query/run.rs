//! `arm query run`: a Resource Graph query as written, over the
//! subscriptions in scope, paged to `--limit`.

use std::path::PathBuf;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use serde_json::Value;

use crate::arm::scrub;
use crate::config::Azure;
use crate::graph::rows;

#[derive(clap::Args)]
pub struct QueryRunArgs {
    /// The KQL, or - to read it from stdin. Resource Graph pages only a query
    /// that projects id
    #[arg(allow_hyphen_values = true, required_unless_present = "kql_file")]
    kql: Option<String>,
    /// The KQL from a file
    #[arg(long)]
    kql_file: Option<PathBuf>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn query_run(ctx: &Ctx, args: QueryRunArgs) -> Result<Vec<Value>> {
    let azure = Azure::load(ctx)?;
    let kql = ctx
        .long_text("kql", args.kql.as_deref(), args.kql_file.as_deref(), None)?
        .map(|kql| kql.text)
        .unwrap_or_default();
    Ok(rows(ctx, &azure.subscriptions, &kql, args.limit)?
        .into_iter()
        .map(scrub)
        .collect())
}

command! {
    pub QUERY_RUN = ["arm", "query", "run"], Read,
    "Run a Resource Graph (KQL) query across the subscriptions in scope",
    keywords: ["kql", "graph", "resourcegraph", "kusto", "securityresources", "count", "summarize"],
    example: "arm query run 'resources | summarize count() by type'",
    run: query_run,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};
    use serde_json::json;

    use crate::testing::SERIAL;

    #[test]
    fn a_piped_query_pages_by_skip_token_to_the_limit_and_masks_secret_text() {
        let transport = FakeTransport::answering([
            Answer::json(&json!({"totalRecords": 3, "data": [
                {"id": "/a", "url": "https://st.blob.core.windows.net/c?sv=2022-11-02&sig=abcdefghijklmnop"}],
                "$skipToken": "page-2"})),
            Answer::json(&json!({"totalRecords": 3, "data": [{"id": "/b"}, {"id": "/c"}]})),
        ]);
        let outcome = run(
            &[crate::ARM],
            &["arm", "query", "run", "-", "--limit", "2"],
            Setup::fake(transport.clone())
                .with_config(&format!(
                    "{SERIAL}subscriptions = \"00000000-0000-4000-8000-000000000001\"\n"
                ))
                .with_stdin("resources | project id"),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let printed = outcome.json();
        assert_eq!(printed[1], json!({"id": "/b"}));
        assert!(
            !outcome.stdout.contains("abcdefghijklmnop"),
            "{}",
            outcome.stdout
        );
        assert!(
            outcome.stderr.contains("[2 of 3; --limit N]"),
            "{}",
            outcome.stderr
        );
        let sent = transport.sent();
        let first = sent[0].body.clone().unwrap();
        assert_eq!(first["query"], "resources | project id");
        assert_eq!(
            first["subscriptions"],
            json!(["00000000-0000-4000-8000-000000000001"])
        );
        assert_eq!(first["options"]["$top"], 3);
        assert_eq!(
            sent[1].body.as_ref().unwrap()["options"]["$skipToken"],
            "page-2"
        );
        assert_eq!(
            sent[0].method,
            agent_cli_core::Method::Query,
            "a read, not a write"
        );
    }

    #[test]
    fn a_kql_mistake_comes_back_in_resource_graphs_words() {
        let transport = FakeTransport::answering([Answer::status(
            400,
            r#"{"error":{"code":"BadRequest","message":"Please provide below info when asking for support","details":[{"code":"InvalidQuery","message":"Query is invalid. Please refer to the documentation for the Azure Resource Graph service and fix the error before retrying."}]}}"#,
        )]);
        let outcome = run(
            &[crate::ARM],
            &["arm", "query", "run", "resources | wher"],
            Setup::fake(transport).with_config(SERIAL),
        );
        assert_eq!(outcome.code, 2, "a 400 is the call to fix: {outcome:?}");
        assert!(
            outcome.stderr.contains("Query is invalid"),
            "{}",
            outcome.stderr
        );
    }
}
