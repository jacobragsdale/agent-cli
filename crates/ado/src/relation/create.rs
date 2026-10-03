use agent_cli_core::{Ctx, Failure, Method, command};
use anyhow::Result;
use serde_json::json;

use crate::client::{Ado, list};
use crate::work_items::moved_on;

use super::{LinkKind, Linked, item_url, linked_id, position, read};

#[derive(clap::Args)]
pub struct RelationCreateArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    #[command(flatten)]
    kind: LinkKind,
}

fn relation_create(ctx: &Ctx, args: RelationCreateArgs) -> Result<Linked> {
    let ado = Ado::load(ctx)?;
    let (id, item, rev) = read(ctx, &ado, &args.id)?;
    let (kind, rel, other) = args.kind.pick();
    // Azure DevOps answers this with a 500.
    if other == id {
        return Err(
            Failure::usage(format!("work item {id} cannot be linked to itself"))
                .hint(format!("agent-cli ado relation create {id} --{kind} OTHER"))
                .into(),
        );
    }
    let linked = |already| Linked {
        work_item: id,
        link: kind,
        other,
        already_linked: Some(already),
        removed: None,
    };
    if position(&item, rel, other).is_some() {
        return Ok(linked(true));
    }
    if kind == "parent"
        && let Some(old) = list(&item["relations"])
            .iter()
            .find(|relation| relation["rel"] == rel)
            .and_then(|relation| linked_id(&relation["url"]))
    {
        return Err(Failure::conflict(format!(
            "work item {id} already has parent {old}, and a work item has one parent"
        ))
        .hint(format!(
            "agent-cli ado relation delete {id} --parent {old}, then run this again"
        ))
        .into());
    }
    // A child's own parent is checked as --parent checks this one's.
    if kind == "child" {
        let (_, child, _) = read(ctx, &ado, &other.to_string())?;
        let up = "System.LinkTypes.Hierarchy-Reverse";
        if let Some(old) = list(&child["relations"])
            .iter()
            .find(|relation| relation["rel"] == up)
            .and_then(|relation| linked_id(&relation["url"]))
        {
            return Err(Failure::conflict(format!(
                "work item {other} already has parent {old}, and a work item has one parent"
            ))
            .hint(format!(
                "agent-cli ado relation delete {other} --parent {old}, then run this again"
            ))
            .into());
        }
    }
    let document = vec![
        json!({"op": "test", "path": "/rev", "value": rev}),
        json!({"op": "add", "path": "/relations/-", "value": {
            "rel": rel,
            "url": format!("{}/_apis/wit/workItems/{other}", ado.base()),
        }}),
    ];
    ado.patch_work_item(ctx, Method::Patch, &item_url(&ado, id), document)
        .map_err(|error| moved_on(error, id))
        .map_err(|error| {
            // TF201035 is a cycle, TF201036 a second link of a one-only kind.
            let said = format!("{error:#}");
            if !said.contains("TF201035") && !said.contains("TF201036") {
                return error;
            }
            Failure::conflict(said)
                .hint(format!(
                    "agent-cli ado workitem get {id} --fields parent,children,related,blocks,blocked_by  (the links in the way)"
                ))
                .into()
        })?;
    Ok(linked(false))
}

command! {
    pub RELATION_CREATE = ["ado", "relation", "create"], Write,
    "Link two work items: parent, child, related, blocks, blocked-by, duplicate-of",
    keywords: ["link", "relate", "mark", "dependency", "depends", "predecessor", "successor", "duplicate", "blocked", "blocker", "under", "connect"],
    example: "ado relation create 42 --blocked-by 41",
    run: relation_create,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, dry_run, item, urls};

    fn linked_item() -> serde_json::Value {
        let mut work = item(42, 7, "Checkout");
        work["relations"] = json!([
            {"rel": "System.LinkTypes.Hierarchy-Reverse", "url": format!("{BASE}/_apis/wit/workItems/7")},
            {"rel": "System.LinkTypes.Dependency-Reverse", "url": format!("{BASE}/_apis/wit/workItems/41")}
        ]);
        work
    }

    #[test]
    fn create_adds_the_relation_behind_a_test_of_the_revision() {
        let plans = dry_run(
            &["ado", "relation", "create", "42", "--blocks", "50"],
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
                {"op": "add", "path": "/relations/-", "value": {
                    "rel": "System.LinkTypes.Dependency-Forward",
                    "url": format!("{BASE}/_apis/wit/workItems/50")}}
            ])
        );

        let (outcome, transport) = ado(
            &["ado", "relation", "create", "42", "--duplicate-of", "9"],
            vec![
                Answer::json(&linked_item()),
                Answer::json(&item(42, 8, "Checkout")),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"work_item": 42, "link": "duplicate-of", "other": 9, "already_linked": false})
        );
        assert_eq!(
            transport.sent()[1].body.as_ref().unwrap()[1]["value"]["rel"],
            "System.LinkTypes.Duplicate-Reverse"
        );
    }

    #[test]
    fn an_existing_link_is_no_change_and_a_second_parent_is_a_conflict() {
        let (outcome, transport) = ado(
            &["ado", "relation", "create", "AB#42", "--blocked-by", "41"],
            vec![Answer::json(&linked_item())],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"work_item": 42, "link": "blocked-by", "other": 41, "already_linked": true})
        );
        assert_eq!(
            urls(&transport),
            [format!(
                "{BASE}/_apis/wit/workitems/42?$expand=relations&api-version=7.1"
            )]
        );

        let (outcome, transport) = ado(
            &["ado", "relation", "create", "42", "--parent", "8"],
            vec![Answer::json(&linked_item())],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli ado relation delete 42 --parent 7, then"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1);

        let (outcome, transport) = ado(
            &["ado", "relation", "create", "42", "--related", "42"],
            vec![Answer::json(&linked_item())],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("cannot be linked to itself"));
        assert_eq!(transport.sent().len(), 1);
    }

    #[test]
    fn exactly_one_kind_is_required() {
        for argv in [
            &["ado", "relation", "create", "42"][..],
            &[
                "ado",
                "relation",
                "create",
                "42",
                "--child",
                "1",
                "--related",
                "2",
            ][..],
        ] {
            let (outcome, transport) = ado(argv, vec![]);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(transport.sent().is_empty());
        }
    }
}
