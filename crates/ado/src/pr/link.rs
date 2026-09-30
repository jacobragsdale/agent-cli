use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, Kind};

use super::{fetch_pr, link_pr, pr_home};

#[derive(clap::Args)]
pub struct PrLinkArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// The work item to link
    #[arg(long)]
    workitem: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrLinked {
    pr: i64,
    work_item: i64,
    /// Nothing was written: the link was already there.
    already_linked: bool,
}

fn pr_link(ctx: &Ctx, args: PrLinkArgs) -> Result<PrLinked> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let pr = fetch_pr(ctx, &ado, id)?;
    let (repo_id, project_id) = pr_home(&pr)?;
    let wrote = link_pr(ctx, &ado, &project_id, &repo_id, id, args.workitem)?;
    Ok(PrLinked {
        pr: id,
        work_item: args.workitem,
        already_linked: !wrote,
    })
}

command! {
    pub PR_LINK = ["ado", "pr", "link"], Write,
    "Link a work item to a pull request",
    keywords: ["attach", "associate", "ticket", "connect", "resolves"],
    example: "ado pr link 17 --workitem 42",
    run: pr_link,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, dry_run, pr};

    #[test]
    fn link_writes_the_artifact_link_on_the_work_item_unless_it_is_there() {
        let work_item = json!({"id": 42, "rev": 5, "fields": {}});
        let plans = dry_run(
            &["ado", "pr", "link", "17", "--workitem", "42"],
            vec![Answer::json(&pr(17, false)), Answer::json(&work_item)],
        );
        assert_eq!(
            plans[0]["url"],
            format!("{BASE}/_apis/wit/workitems/42?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!([{"op": "test", "path": "/rev", "value": 5},
                {"op": "add", "path": "/relations/-", "value": {"rel": "ArtifactLink",
                    "url": "vstfs:///Git/PullRequestId/p-1%2Fr-1%2F17", "attributes": {"name": "Pull Request"}}}])
        );

        let linked = json!({"id": 42, "rev": 5, "relations": [
            {"rel": "ArtifactLink", "url": "vstfs:///Git/PullRequestId/p-1%2fr-1%2f17"}]});
        let (outcome, transport) = ado(
            &["ado", "pr", "link", "17", "--workitem", "42"],
            vec![Answer::json(&pr(17, false)), Answer::json(&linked)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"pr": 17, "work_item": 42, "already_linked": true})
        );
        assert!(transport.sent().iter().all(|sent| sent.method.is_read()));
    }
}
