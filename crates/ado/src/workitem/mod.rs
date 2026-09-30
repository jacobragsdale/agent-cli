//! Work items: the WIQL-backed list, one work item with its links and latest
//! comments, and the writes (create, update, comment, link to a branch).

pub(crate) mod comment;
pub(crate) mod create;
pub(crate) mod get;
pub(crate) mod link;
pub(crate) mod list;
pub(crate) mod update;

use std::path::PathBuf;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::Ado;
use crate::markdown::markdown_to_html;

/// The link type a parent is held under, on the child's side.
const PARENT: &str = "System.LinkTypes.Hierarchy-Reverse";

// ---------- the fields create and update write ----------

#[derive(clap::Args)]
struct Fields {
    /// Active, Closed …
    #[arg(long)]
    state: Option<String>,
    /// Name, email or @me ("" unassigns)
    #[arg(long)]
    assignee: Option<String>,
    /// Full iteration path
    #[arg(long)]
    iteration: Option<String>,
    /// Full area path
    #[arg(long)]
    area: Option<String>,
    /// 1 (highest) to 4
    #[arg(long)]
    priority: Option<i64>,
    /// Comma-separated; replaces the tags it has
    #[arg(long)]
    tags: Option<String>,
    /// Markdown, stored as HTML; - reads stdin
    #[arg(long, allow_hyphen_values = true)]
    description: Option<String>,
    /// The description from a Markdown file
    #[arg(long)]
    description_file: Option<PathBuf>,
    /// Markdown, stored as HTML; - reads stdin
    #[arg(long, allow_hyphen_values = true)]
    acceptance_criteria: Option<String>,
    /// The acceptance criteria from a Markdown file
    #[arg(long)]
    acceptance_criteria_file: Option<PathBuf>,
}

/// One JSON Patch operation setting a field. Azure DevOps takes `add` for a
/// field that is already set as well as for one that is not.
fn set(field: &str, value: impl Into<Value>) -> Value {
    json!({"op": "add", "path": format!("/fields/{field}"), "value": value.into()})
}

/// The operations that write `fields`, in the order the flags are documented.
fn field_ops(ctx: &Ctx, ado: &Ado, title: Option<&str>, fields: &Fields) -> Result<Vec<Value>> {
    let mut ops = Vec::new();
    if let Some(title) = title {
        let title = title.trim();
        if title.is_empty() {
            return Err(Failure::usage("a work item cannot have an empty title").into());
        }
        ops.push(set("System.Title", title));
    }
    if let Some(state) = &fields.state {
        ops.push(set("System.State", state.trim()));
    }
    if let Some(who) = &fields.assignee {
        let who = who.trim();
        ops.push(if who.is_empty() {
            // Nobody is written by removing the field, not by an empty name.
            json!({"op": "remove", "path": "/fields/System.AssignedTo"})
        } else if who.eq_ignore_ascii_case("@me") {
            let me = ado.me(ctx)?;
            set("System.AssignedTo", me.account.unwrap_or(me.name))
        } else {
            set("System.AssignedTo", who)
        });
    }
    if let Some(iteration) = &fields.iteration {
        ops.push(set("System.IterationPath", iteration.trim()));
    }
    if let Some(area) = &fields.area {
        ops.push(set("System.AreaPath", area.trim()));
    }
    if let Some(priority) = fields.priority {
        ops.push(set("Microsoft.VSTS.Common.Priority", priority));
    }
    if let Some(tags) = &fields.tags {
        ops.push(set("System.Tags", tag_list(tags)));
    }
    if fields.description.as_deref() == Some("-")
        && fields.acceptance_criteria.as_deref() == Some("-")
    {
        return Err(Failure::usage(
            "--description and --acceptance-criteria cannot both read stdin",
        )
        .hint("pipe one, and pass the other as --acceptance-criteria-file PATH")
        .into());
    }
    for (name, field, typed, file) in [
        (
            "description",
            "System.Description",
            &fields.description,
            &fields.description_file,
        ),
        (
            "acceptance-criteria",
            "Microsoft.VSTS.Common.AcceptanceCriteria",
            &fields.acceptance_criteria,
            &fields.acceptance_criteria_file,
        ),
    ] {
        if let Some(markdown) = ctx.long_text(name, typed.as_deref(), file.as_deref(), None)? {
            ops.push(set(field, markdown_to_html(&markdown.text)));
        }
    }
    Ok(ops)
}

/// `a, b,A` as `System.Tags` holds it: `a; b`, each tag once whatever its case.
fn tag_list(raw: &str) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut tags: Vec<&str> = Vec::new();
    for tag in raw
        .split([',', ';'])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
    {
        let folded = tag.to_lowercase();
        if !seen.contains(&folded) {
            seen.push(folded);
            tags.push(tag);
        }
    }
    tags.join("; ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{ado_piped, dry_run};

    /// What `markdown_to_html` makes of `markdown`, so the tests above do not
    /// restate the renderer.
    fn markdown_html(markdown: &str) -> String {
        crate::markdown::markdown_to_html(markdown)
    }

    #[test]
    fn long_text_comes_typed_piped_with_a_dash_or_from_a_file() {
        let (outcome, transport) = ado_piped(
            "## Why\n\n- customers ask\n",
            &[
                "ado",
                "workitem",
                "create",
                "--type",
                "Bug",
                "--title",
                "Crash",
                "--description",
                "-",
                "--dry-run",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(transport.sent().is_empty());
        assert_eq!(
            outcome.json()["would"][0]["body"][1],
            json!({"op": "add", "path": "/fields/System.Description",
                "value": markdown_html("## Why\n\n- customers ask")})
        );

        let dir = tempfile::tempdir().unwrap();
        let criteria = dir.path().join("criteria.md");
        std::fs::write(&criteria, "- works on Safari\n").unwrap();
        let criteria = criteria.to_str().unwrap();
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "update",
                "42",
                "--description",
                "- a bullet, not a flag",
                "--acceptance-criteria-file",
                criteria,
            ],
            vec![],
        );
        assert_eq!(
            plans[0]["body"],
            json!([
                {"op": "add", "path": "/fields/System.Description", "value": markdown_html("- a bullet, not a flag")},
                {"op": "add", "path": "/fields/Microsoft.VSTS.Common.AcceptanceCriteria", "value": markdown_html("- works on Safari")}
            ])
        );

        for (stdin, argv, said) in [
            (
                "x",
                &[
                    "ado",
                    "workitem",
                    "update",
                    "42",
                    "--description",
                    "-",
                    "--acceptance-criteria",
                    "-",
                ][..],
                "cannot both read stdin",
            ),
            (
                "x",
                &[
                    "ado",
                    "workitem",
                    "update",
                    "42",
                    "--description",
                    "x",
                    "--description-file",
                    criteria,
                ][..],
                "both as a value and as the file",
            ),
            (
                "",
                &["ado", "workitem", "update", "42", "--description", "-"][..],
                "the description from stdin is empty",
            ),
            (
                "x",
                &[
                    "ado",
                    "workitem",
                    "update",
                    "42",
                    "--acceptance-criteria-file",
                    "/nonexistent.md",
                ][..],
                "cannot read the acceptance criteria from /nonexistent.md",
            ),
        ] {
            let (outcome, transport) = ado_piped(stdin, argv, vec![]);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(outcome.stderr.contains(said), "{}", outcome.stderr);
            assert!(transport.sent().is_empty());
        }
    }
}
