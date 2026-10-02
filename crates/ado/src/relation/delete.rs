use agent_cli_core::{Ctx, Failure, Method, command};
use anyhow::Result;
use serde_json::json;

use crate::client::Ado;
use crate::work_items::moved_on;

use super::{LinkKind, Linked, item_url, position, read};

#[derive(clap::Args)]
pub struct RelationDeleteArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    #[command(flatten)]
    kind: LinkKind,
}

fn relation_delete(ctx: &Ctx, args: RelationDeleteArgs) -> Result<Linked> {
    let ado = Ado::load(ctx)?;
    let (id, item, rev) = read(ctx, &ado, &args.id)?;
    let (kind, rel, other) = args.kind.pick();
    let Some(at) = position(&item, rel, other) else {
        return Err(Failure::not_found(format!(
            "work item {id} has no {kind} link to {other}"
        ))
        .hint(format!(
            "agent-cli ado workitem get {id} --fields parent,children,related,blocks,blocked_by"
        ))
        .into());
    };
    // The test keeps the index honest: a link added or removed since the read
    // shifts the others.
    let document = vec![
        json!({"op": "test", "path": "/rev", "value": rev}),
        json!({"op": "remove", "path": format!("/relations/{at}")}),
    ];
    ado.patch_work_item(ctx, Method::Patch, &item_url(&ado, id), document)
        .map_err(|error| moved_on(error, id))?;
    Ok(Linked {
        work_item: id,
        link: kind,
        other,
        already_linked: None,
        removed: Some(true),
    })
}

command! {
    pub RELATION_DELETE = ["ado", "relation", "delete"], Write,
    "Unlink two work items: remove a parent, child, related or blocking link",
    keywords: ["unlink", "remove", "dependency", "detach", "predecessor", "successor", "duplicate", "hierarchy", "blocker"],
    example: "ado relation delete 42 --parent 7",
    run: relation_delete,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, dry_run, item};

    fn linked_item() -> serde_json::Value {
        let mut work = item(42, 7, "Checkout");
        work["relations"] = json!([
            {"rel": "ArtifactLink", "url": "vstfs:///Git/PullRequestId/p-1%2Fr-1%2F17"},
            {"rel": "System.LinkTypes.Hierarchy-Reverse", "url": format!("{BASE}/_apis/wit/workItems/7")},
            {"rel": "System.LinkTypes.Dependency-Forward", "url": format!("{BASE}/_apis/wit/workItems/50")}
        ]);
        work
    }

    #[test]
    fn delete_removes_the_link_by_its_index_behind_a_test_of_the_revision() {
        let plans = dry_run(
            &["ado", "relation", "delete", "42", "--blocks", "50"],
            vec![Answer::json(&linked_item())],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            format!("{BASE}/_apis/wit/workitems/42?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!([
                {"op": "test", "path": "/rev", "value": 7},
                {"op": "remove", "path": "/relations/2"}
            ])
        );

        let (outcome, _) = ado(
            &["ado", "relation", "delete", "42", "--parent", "7"],
            vec![
                Answer::json(&linked_item()),
                Answer::json(&item(42, 8, "Checkout")),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"work_item": 42, "link": "parent", "other": 7, "removed": true})
        );
    }

    #[test]
    fn a_missing_link_is_4_and_a_changed_item_is_5() {
        let (outcome, transport) = ado(
            &["ado", "relation", "delete", "42", "--blocked-by", "50"],
            vec![Answer::json(&linked_item())],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("has no blocked-by link to 50"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1);

        let (outcome, _) = ado(
            &["ado", "relation", "delete", "42", "--parent", "7"],
            vec![
                Answer::json(&linked_item()),
                Answer::status(
                    412,
                    r#"{"message":"TF401289: The test operation failed for /rev."}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome.stderr.contains("changed since it was read"),
            "{}",
            outcome.stderr
        );
    }
}
