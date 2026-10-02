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
use crate::compose::{COMMENT_LIMIT, rich_text};
use crate::types;

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
    /// Any other field by reference or display name, repeatable; NAME= clears it
    #[arg(long = "field", value_name = "NAME=VALUE")]
    field: Vec<String>,
}

/// `update`'s comment, which goes in the same patch as its field changes:
/// one revision, one notification.
#[derive(clap::Args)]
struct Comment {
    /// Markdown, sent with the change; @<Name> mentions; - reads stdin
    #[arg(long, allow_hyphen_values = true)]
    comment: Option<String>,
    /// The comment from a Markdown file
    #[arg(long)]
    comment_file: Option<PathBuf>,
}

/// The fields a typed flag writes, so `--field` cannot write one again.
const TYPED: &[(&str, &str)] = &[
    ("System.Title", "--title"),
    ("System.State", "--state"),
    ("System.AssignedTo", "--assignee"),
    ("System.IterationPath", "--iteration"),
    ("System.AreaPath", "--area"),
    ("Microsoft.VSTS.Common.Priority", "--priority"),
    ("System.Tags", "--tags"),
    ("System.Description", "--description"),
    (
        "Microsoft.VSTS.Common.AcceptanceCriteria",
        "--acceptance-criteria",
    ),
    ("System.History", "--comment"),
];

/// The codes Azure DevOps refuses a patch with when it breaks the type's
/// rules: TF401320 a required or limited field, TF401326 a value the field
/// does not allow, TF51535 a field the type lacks. A state or picklist value
/// outside the list comes back live with no code, only these words.
const RULE_ERRORS: &[&str] = &[
    "TF401320",
    "TF401326",
    "TF51535",
    "not in the list of supported values",
];

/// One JSON Patch operation setting a field. Azure DevOps takes `add` for a
/// field that is already set as well as for one that is not.
fn set(field: &str, value: impl Into<Value>) -> Value {
    json!({"op": "add", "path": format!("/fields/{field}"), "value": value.into()})
}

/// The operations that write `fields` and the comment, in the order the
/// flags are documented. `kind` is the work item's type, asked for only when
/// `--field` needs its fields.
fn field_ops(
    ctx: &Ctx,
    ado: &Ado,
    title: Option<&str>,
    fields: &Fields,
    comment: Option<&Comment>,
    kind: &dyn Fn() -> Result<String>,
) -> Result<Vec<Value>> {
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
            set("System.AssignedTo", me.email.unwrap_or(me.name))
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
    let mut long = vec![
        (
            "description",
            "System.Description",
            &fields.description,
            &fields.description_file,
            None,
        ),
        (
            "acceptance-criteria",
            "Microsoft.VSTS.Common.AcceptanceCriteria",
            &fields.acceptance_criteria,
            &fields.acceptance_criteria_file,
            None,
        ),
    ];
    if let Some(comment) = comment {
        long.push((
            "comment",
            "System.History",
            &comment.comment,
            &comment.comment_file,
            Some(COMMENT_LIMIT),
        ));
    }
    let piped: Vec<String> = long
        .iter()
        .filter(|(_, _, typed, _, _)| typed.as_deref() == Some("-"))
        .map(|(name, ..)| format!("--{name}"))
        .collect();
    if let [first, second, ..] = piped.as_slice() {
        return Err(
            Failure::usage(format!("{first} and {second} cannot both read stdin"))
                .hint(format!(
                    "pipe one, and pass the other as {second}-file PATH"
                ))
                .into(),
        );
    }
    for (name, field, typed, file, limit) in long {
        if let Some(markdown) = ctx.long_text(name, typed.as_deref(), file.as_deref(), limit)? {
            if field == "System.History" && markdown.text.trim().is_empty() {
                return Err(Failure::usage("a comment cannot be empty").into());
            }
            ops.push(set(field, rich_text(ctx, ado, &markdown.text)?));
        }
    }
    if !fields.field.is_empty() {
        let more = custom_ops(ctx, ado, &kind()?, &fields.field, &ops)?;
        ops.extend(more);
    }
    Ok(ops)
}

/// `--field NAME=VALUE`s as operations. NAME is found among `kind`'s fields
/// by reference name, else by display name; a numeric field gets a number,
/// an HTML one Markdown as HTML, and an empty VALUE removes the field.
fn custom_ops(
    ctx: &Ctx,
    ado: &Ado,
    kind: &str,
    pairs: &[String],
    typed: &[Value],
) -> Result<Vec<Value>> {
    let fields = types::fields(ctx, ado, kind)?;
    let mut ops: Vec<Value> = Vec::new();
    for pair in pairs {
        let Some((name, value)) = pair.split_once('=') else {
            return Err(
                Failure::usage(format!("--field {pair:?} is not NAME=VALUE"))
                    .hint("--field \"Story Points=5\", or NAME= to clear it")
                    .into(),
            );
        };
        let (name, value) = (name.trim(), value.trim());
        let Some(field) = fields
            .iter()
            .find(|field| field.reference.eq_ignore_ascii_case(name))
            .or_else(|| {
                fields
                    .iter()
                    .find(|field| field.name.eq_ignore_ascii_case(name))
            })
        else {
            return Err(Failure::usage(format!("{kind} has no field {name:?}"))
                .hint(type_hint(kind))
                .into());
        };
        let path = format!("/fields/{}", field.reference);
        if typed.iter().chain(&ops).any(|op| op["path"] == path) {
            let flag = TYPED
                .iter()
                .find(|(reference, _)| *reference == field.reference)
                .map_or("another --field", |(_, flag)| flag);
            return Err(Failure::usage(format!(
                "--field {name} and {flag} both set {}",
                field.reference
            ))
            .hint("pass it once")
            .into());
        }
        if value.is_empty() {
            ops.push(json!({"op": "remove", "path": path}));
            continue;
        }
        let number = |parsed: Option<Value>| {
            parsed.ok_or_else(|| {
                Failure::usage(format!(
                    "{} ({}) takes a number, not {value:?}",
                    field.name, field.reference
                ))
            })
        };
        let value = match field.kind.as_deref() {
            Some("integer" | "picklistInteger") => {
                number(value.parse::<i64>().ok().map(Value::from))?
            }
            Some("double" | "picklistDouble") => {
                number(value.parse::<f64>().ok().map(Value::from))?
            }
            Some("html") => Value::from(rich_text(ctx, ado, value)?),
            _ => Value::from(value),
        };
        ops.push(set(&field.reference, value));
    }
    Ok(ops)
}

/// A refusal over the type's rules (a state it lacks, a required field, a
/// value it does not allow) in Azure DevOps's own words, pointing at the
/// rules. `kind` is asked for only then.
fn broke_rules(error: anyhow::Error, kind: impl FnOnce() -> String) -> anyhow::Error {
    let Some(failure) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<Failure>())
    else {
        return error;
    };
    if failure.status != Some(400)
        || !RULE_ERRORS
            .iter()
            .any(|code| failure.message.contains(code))
    {
        return error;
    }
    let mut kept = Failure::new(failure.exit, failure.message.clone()).hint(type_hint(&kind()));
    kept.status = failure.status;
    kept.into()
}

/// The command that shows a type's states and fields.
fn type_hint(kind: &str) -> String {
    let kind = kind.trim();
    if kind.contains(char::is_whitespace) {
        format!("agent-cli ado workitem-type get {kind:?}")
    } else {
        format!("agent-cli ado workitem-type get {kind}")
    }
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
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, CODE, ado, ado_piped, dry_run, page, person, story_fields, urls};

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

    fn story(answers: Vec<Answer>) -> Vec<Answer> {
        let mut all = vec![Answer::json(
            &json!({"id": 1207, "fields": {"System.WorkItemType": "User Story"}}),
        )];
        all.extend(answers);
        all
    }

    #[test]
    fn field_sets_any_field_by_reference_or_display_name_with_numbers_as_numbers() {
        let argv = [
            "ado",
            "workitem",
            "update",
            "1207",
            "--state",
            "Active",
            "--field",
            "story points=5",
            "--field",
            "Microsoft.VSTS.Common.Risk=",
            "--field",
            "Repro Steps=**boom** on save",
            "--dry-run",
        ];
        let (outcome, transport) = ado(&argv, story(story_fields()));
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["would"][0]["body"],
            json!([
                {"op": "add", "path": "/fields/System.State", "value": "Active"},
                {"op": "add", "path": "/fields/Microsoft.VSTS.Scheduling.StoryPoints", "value": 5.0},
                {"op": "remove", "path": "/fields/Microsoft.VSTS.Common.Risk"},
                {"op": "add", "path": "/fields/Microsoft.VSTS.TCM.ReproSteps",
                    "value": "<p><b>boom</b> on save</p>"}
            ])
        );
        assert_eq!(
            urls(&transport),
            [
                format!(
                    "{BASE}/_apis/wit/workitems/1207?fields=System.WorkItemType&api-version=7.1"
                ),
                format!(
                    "{CODE}/wit/workitemtypes/User%20Story/fields?$expand=allowedValues&api-version=7.1"
                ),
                format!("{CODE}/wit/fields?api-version=7.1"),
            ]
        );

        // create knows the type, so it reads nothing about the item.
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "create",
                "--type",
                "User Story",
                "--title",
                "Pay",
                "--field",
                "Story Points=3",
            ],
            story_fields(),
        );
        assert_eq!(
            plans[0]["body"][1],
            json!({"op": "add", "path": "/fields/Microsoft.VSTS.Scheduling.StoryPoints", "value": 3.0})
        );
    }

    #[test]
    fn a_field_the_type_lacks_or_a_typed_flag_sets_is_exit_2_before_anything_is_sent() {
        for (fields, said, hint) in [
            (
                &["--field", "Nope=1"][..],
                "User Story has no field \"Nope\"",
                Some("hint: agent-cli ado workitem-type get \"User Story\""),
            ),
            (
                &["--state", "Active", "--field", "system.state=Closed"][..],
                "--field system.state and --state both set System.State",
                None,
            ),
            (
                &[
                    "--field",
                    "Story Points=1",
                    "--field",
                    "Microsoft.VSTS.Scheduling.StoryPoints=2",
                ][..],
                "and another --field both set",
                None,
            ),
            (
                &["--field", "Story Points=lots"][..],
                "Story Points (Microsoft.VSTS.Scheduling.StoryPoints) takes a number, not \"lots\"",
                None,
            ),
            (&["--field", "Story Points"][..], "is not NAME=VALUE", None),
        ] {
            let mut argv = vec![
                "ado",
                "workitem",
                "create",
                "--type",
                "User Story",
                "--title",
                "Pay",
            ];
            argv.extend(fields);
            let (outcome, transport) = ado(&argv, story_fields());
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(outcome.stderr.contains(said), "{}", outcome.stderr);
            if let Some(hint) = hint {
                assert!(outcome.stderr.contains(hint), "{}", outcome.stderr);
            }
            assert!(transport.sent().iter().all(|sent| sent.method.is_read()));
        }
    }

    #[test]
    fn comment_goes_in_the_same_patch_as_history_with_its_mentions_resolved() {
        let me = Answer::json(&json!({"authenticatedUser": {"id": "u-1",
            "providerDisplayName": "Jane Doe", "properties": {"Account": {"$value": "jane@contoso.com"}}}}));
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "update",
                "1207",
                "--assignee",
                "@me",
                "--comment",
                "Taking this, @<Sam Lee>",
                "--if-rev",
                "7",
            ],
            vec![me, person("u-2", "Sam Lee", "sam@contoso.com")],
        );
        assert_eq!(
            plans[0]["body"],
            json!([
                {"op": "test", "path": "/rev", "value": 7},
                {"op": "add", "path": "/fields/System.AssignedTo", "value": "jane@contoso.com"},
                {"op": "add", "path": "/fields/System.History", "value":
                    "<p>Taking this, <a href=\"#\" data-vss-mention=\"version:2.0,u-2\">@Sam Lee</a></p>"}
            ])
        );

        let (outcome, _) = ado_piped(
            "Fixed in **!17**\n",
            &[
                "ado",
                "workitem",
                "update",
                "1207",
                "--comment",
                "-",
                "--dry-run",
            ],
            vec![],
        );
        assert_eq!(
            outcome.json()["would"][0]["body"],
            json!([{"op": "add", "path": "/fields/System.History", "value": markdown_html("Fixed in **!17**")}])
        );

        for (argv, said) in [
            (
                &[
                    "ado",
                    "workitem",
                    "update",
                    "1207",
                    "--description",
                    "-",
                    "--comment",
                    "-",
                ][..],
                "--description and --comment cannot both read stdin",
            ),
            (
                &["ado", "workitem", "update", "1207", "--comment", " "][..],
                "a comment cannot be empty",
            ),
        ] {
            let (outcome, transport) = ado_piped("x", argv, vec![]);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(outcome.stderr.contains(said), "{}", outcome.stderr);
            assert!(transport.sent().is_empty());
        }

        // Nobody by that name: exit 4, and the change is never sent.
        let (outcome, transport) = ado(
            &[
                "ado",
                "workitem",
                "update",
                "1207",
                "--description",
                "ask @<Nobody>",
            ],
            vec![page(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(transport.sent().iter().all(|sent| sent.method.is_read()));
    }
}
