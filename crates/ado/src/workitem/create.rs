use agent_cli_core::{Ctx, Method, command};
use anyhow::Result;
use serde_json::json;

use crate::client::{Ado, segment};
use crate::work_items::{WorkItemRow, row};

use super::{Fields, PARENT, field_ops};

#[derive(clap::Args)]
pub struct CreateArgs {
    /// Bug, Task, "User Story" …
    #[arg(long = "type")]
    work_item_type: String,
    /// What it is called
    #[arg(long)]
    title: String,
    /// The work item it goes under
    #[arg(long)]
    parent: Option<i64>,
    #[command(flatten)]
    fields: Fields,
}

fn workitem_create(ctx: &Ctx, args: CreateArgs) -> Result<WorkItemRow> {
    let ado = Ado::load(ctx)?;
    let mut document = field_ops(ctx, &ado, Some(&args.title), &args.fields)?;
    if let Some(parent) = args.parent {
        // A parent is a link appended to the new work item, not a field.
        document.push(json!({
            "op": "add",
            "path": "/relations/-",
            "value": {
                "rel": PARENT,
                "url": format!("{}/_apis/wit/workItems/{parent}", ado.base()),
            },
        }));
    }
    // The type is a path segment with a literal `$` in front of it.
    let url = ado.work(
        &format!("wit/workitems/${}", segment(args.work_item_type.trim())),
        "",
    );
    let created = ado.patch_work_item(ctx, Method::Post, &url, document)?;
    Ok(row(&created))
}

command! {
    pub WORKITEM_CREATE = ["ado", "workitem", "create"], Write,
    "Create a work item (bug, task, story …), optionally under a parent",
    keywords: ["new", "file", "open", "add", "ticket", "bug", "story"],
    example: "ado workitem create --type Bug --title 'Login fails on Safari' --priority 2",
    run: workitem_create,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, dry_run, item};

    #[test]
    fn create_plans_a_patch_document_with_markdown_as_html_and_the_parent_link() {
        let me = Answer::json(&json!({"authenticatedUser": {"id": "u-1",
            "providerDisplayName": "Jane Doe", "properties": {"Account": {"$value": "jane@contoso.com"}}}}));
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "create",
                "--type",
                "User Story",
                "--title",
                " Pay by card ",
                "--assignee",
                "@me",
                "--parent",
                "7",
                "--tags",
                "ui,UI, p1",
                "--description",
                "**Why**: customers ask",
            ],
            vec![me],
        );
        assert_eq!(plans.len(), 1);
        let plan = &plans[0];
        assert_eq!(plan["method"], "POST");
        assert_eq!(
            plan["url"],
            format!("{BASE}/Fabrikam/_apis/wit/workitems/$User%20Story?api-version=7.1")
        );
        assert_eq!(
            plan["headers"]["Content-Type"],
            "application/json-patch+json"
        );
        assert_eq!(
            plan["body"],
            json!([
                {"op": "add", "path": "/fields/System.Title", "value": "Pay by card"},
                {"op": "add", "path": "/fields/System.AssignedTo", "value": "jane@contoso.com"},
                {"op": "add", "path": "/fields/System.Tags", "value": "ui; p1"},
                {"op": "add", "path": "/fields/System.Description", "value": "<p><b>Why</b>: customers ask</p>"},
                {"op": "add", "path": "/relations/-", "value": {
                    "rel": "System.LinkTypes.Hierarchy-Reverse",
                    "url": format!("{BASE}/_apis/wit/workItems/7")}}
            ])
        );
        assert!(!plan.to_string().contains("fixture-pat"));

        let (outcome, transport) = ado(
            &[
                "ado", "workitem", "create", "--type", "Bug", "--title", "Crash",
            ],
            vec![Answer::json(&item(77, 1, "Crash"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["id"], 77);
        assert!(!transport.sent()[0].method.is_read());
    }
}
