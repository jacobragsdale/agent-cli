use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Failure, Method, command, status_of};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::{Confluence, Content, text};
use crate::compose::{Target, to_storage};
use crate::ids::current_page;
use crate::markdown::{Context, mentions, to_markdown};
use crate::storage::{find, parse, sections};

use super::{BODY_LIMIT, Written, labels, people};

#[derive(clap::Args)]
pub struct PageUpdateArgs {
    /// The page: its id, KEY:Title or its URL
    page: String,
    /// A new title (alone, it never touches the body)
    #[arg(long)]
    title: Option<String>,
    /// The new body, Markdown or - to read stdin: the whole page (needs --if-version), or --section's from its heading
    #[arg(long, allow_hyphen_values = true)]
    body: Option<String>,
    /// The new body from a file
    #[arg(long)]
    body_file: Option<PathBuf>,
    /// Markdown to add at the end of the page, or of --section; - reads stdin
    #[arg(long, allow_hyphen_values = true)]
    append: Option<String>,
    /// What to append, from a file
    #[arg(long)]
    append_file: Option<PathBuf>,
    /// The section --body replaces or --append extends: its heading's text, any case
    #[arg(long)]
    section: Option<String>,
    /// Move the page under this page id, in any space
    #[arg(long)]
    parent: Option<u64>,
    /// The labels it should have, comma-separated (replaces them; "" removes all)
    #[arg(long)]
    labels: Option<String>,
    /// The version comment
    #[arg(long)]
    message: Option<String>,
    /// --body or --append is storage XHTML, sent as it is
    #[arg(long)]
    storage: bool,
    /// Refuse unless the page is still at this version (page get prints it)
    #[arg(long)]
    if_version: Option<u64>,
}

fn page_update(ctx: &Ctx, args: PageUpdateArgs) -> Result<Written> {
    let confluence = Confluence::load(ctx)?;
    let body = ctx.long_text(
        "body",
        args.body.as_deref(),
        args.body_file.as_deref(),
        Some(BODY_LIMIT),
    )?;
    let append = ctx.long_text(
        "append",
        args.append.as_deref(),
        args.append_file.as_deref(),
        Some(BODY_LIMIT),
    )?;
    let edits_body = body.is_some() || append.is_some();
    let kinds = [
        edits_body || args.title.is_some(),
        args.parent.is_some(),
        args.labels.is_some(),
    ];
    let usage = |message: &str| -> anyhow::Error { Failure::usage(message.to_owned()).into() };
    match kinds.iter().filter(|kind| **kind).count() {
        0 => {
            return Err(usage(
                "nothing to change: give --title, --body, --append, --parent or --labels",
            ));
        }
        1 => {}
        _ => {
            return Err(usage(
                "one kind of change a call: a title and body, a move (--parent), or labels",
            ));
        }
    }
    if body.is_some() && append.is_some() {
        return Err(usage("--body replaces and --append adds: give one"));
    }
    if !edits_body && (args.section.is_some() || args.storage || args.message.is_some()) {
        return Err(usage(
            "--section, --storage and --message go with --body or --append",
        ));
    }
    let id = current_page(ctx, &confluence, &args.page)?;
    if body.is_some() && args.section.is_none() && args.if_version.is_none() {
        return Err(Failure::usage(
            "replacing the whole body needs --if-version N, so a change made since you read it is not overwritten",
        )
        .hint(format!("agent-cli confluence page get {id} --fields version"))
        .into());
    }
    let content = confluence.content(
        ctx,
        &id,
        &[
            ("body-format", "storage".to_owned()),
            ("include-labels", "true".to_owned()),
        ],
    )?;
    let version = content.version();
    if let Some(wanted) = args.if_version
        && wanted != version
    {
        return Err(changed(&id, wanted, version));
    }
    let space_id = text(&content.value["spaceId"]).unwrap_or_default();
    let key = confluence.space_key(ctx, &space_id, &mut Default::default())?;
    let mut written = Written {
        id: id.clone(),
        title: text(&content.value["title"]),
        space: Some(key.clone()),
        version: Some(version),
        url: confluence.web_of(&content.value),
    };
    if let Some(parent) = args.parent {
        let url = confluence.v1(&format!("/content/{id}/move/append/{parent}"), &[]);
        confluence.change(ctx, Effect::Write, Method::Put, &url, None)?;
        let moved = confluence.content(ctx, &id, &[])?;
        let space_id = text(&moved.value["spaceId"]).unwrap_or_default();
        written.space = Some(confluence.space_key(ctx, &space_id, &mut Default::default())?);
        written.url = confluence.web_of(&moved.value);
        return Ok(written);
    }
    if let Some(wanted) = args.labels.as_deref().map(labels) {
        relabel(ctx, &confluence, &content, &wanted)?;
        return Ok(written);
    }
    let old = content.value["body"]["storage"]["value"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let title = args.title.clone().or_else(|| text(&content.value["title"]));
    let new = match (&body, &append) {
        (None, None) if content.kind == "pages" => {
            // A title alone never resends the body, which another tool's
            // stale copy once wiped pages with.
            let url = confluence.v2(&format!("/pages/{id}/title"), &[]);
            let answer = confluence
                .change(
                    ctx,
                    Effect::Write,
                    Method::Put,
                    &url,
                    Some(json!({"status": "current", "title": title})),
                )
                .map_err(|error| conflict(error, &id, version))?;
            written.title = text(&answer["title"]).or(title);
            written.version = answer["version"]["number"].as_u64().or(Some(version + 1));
            return Ok(written);
        }
        (None, None) => old,
        (Some(text), _) | (_, Some(text)) => {
            let nodes = parse(&old);
            let range = match &args.section {
                Some(name) => find(&sections(&old, &nodes), name, &id)?.range.clone(),
                None => 0..old.len(),
            };
            let part = if args.storage {
                text.text.clone()
            } else {
                let people = people(ctx, &confluence, &text.text, &mentions(&nodes))?;
                to_storage(
                    &text.text,
                    &Target {
                        space: &key,
                        people: &people,
                    },
                )
            };
            if body.is_some() {
                if !args.storage {
                    gate(&old[range.clone()], &id, version, args.section.is_some())?;
                }
                format!("{}{part}{}", &old[..range.start], &old[range.end..])
            } else {
                format!("{}{part}{}", &old[..range.end], &old[range.end..])
            }
        }
    };
    let mut next = json!({"number": version + 1});
    if let Some(message) = &args.message {
        next["message"] = json!(message);
    }
    let url = confluence.v2(&format!("/{}/{id}", content.kind), &[]);
    let request = json!({
        "id": id, "status": "current", "title": title,
        "body": {"representation": "storage", "value": new},
        "version": next,
    });
    let answer = confluence
        .change(ctx, Effect::Write, Method::Put, &url, Some(request))
        .map_err(|error| conflict(error, &id, version))?;
    written.title = text(&answer["title"]).or(title);
    written.version = answer["version"]["number"].as_u64().or(Some(version + 1));
    Ok(written)
}

/// The loss gate: what the part being replaced holds that Markdown cannot
/// carry (macros, layouts, inline comment marks) is not overwritten.
fn gate(part: &str, id: &str, version: u64, section: bool) -> Result<()> {
    let names = std::collections::HashMap::new();
    let read = to_markdown(
        &parse(part),
        &Context {
            space: "",
            names: &names,
        },
    );
    if read.lossy.is_empty() {
        return Ok(());
    }
    let what = if section { "the section" } else { "the page" };
    Err(Failure::usage(format!(
        "{what} holds what Markdown cannot carry ({}); replacing it with Markdown would lose them",
        read.lossy.join(", ")
    ))
    .hint(format!(
        "edit a smaller --section, use --append, or edit the storage: agent-cli confluence page get {id} --storage --output page.xml, then agent-cli confluence page update {id} --storage --body-file page.xml --if-version {version}"
    ))
    .into())
}

fn changed(id: &str, wanted: u64, now: u64) -> anyhow::Error {
    Failure::conflict(format!(
        "page {id} changed since version {wanted} (now {now})"
    ))
    .hint(format!("agent-cli confluence page get {id}"))
    .into()
}

/// A 409 that names the current version is a change made since the read;
/// any other 409 (a space that requires approval) keeps the service's words.
fn conflict(error: anyhow::Error, id: &str, read: u64) -> anyhow::Error {
    if status_of(&error) != Some(409) {
        return error;
    }
    let said = format!("{error:#}");
    match said
        .split_once("Current Version: [")
        .and_then(|(_, rest)| rest.split(']').next())
        .and_then(|now| now.parse::<u64>().ok())
    {
        Some(now) => changed(id, read, now),
        None => error,
    }
}

/// Labels as asked: v1 adds the missing ones in one call and removes the
/// rest one by one. Labels make no new version.
fn relabel(ctx: &Ctx, confluence: &Confluence, content: &Content, wanted: &[String]) -> Result<()> {
    let id = text(&content.value["id"]).unwrap_or_default();
    let have: Vec<String> = content.value["labels"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|label| text(&label["name"]))
        .collect();
    let add: Vec<Value> = wanted
        .iter()
        .filter(|name| !have.contains(name))
        .map(|name| json!({"prefix": "global", "name": name}))
        .collect();
    if !add.is_empty() {
        let url = confluence.v1(&format!("/content/{id}/label"), &[]);
        confluence.change(
            ctx,
            Effect::Write,
            Method::Post,
            &url,
            Some(Value::Array(add)),
        )?;
    }
    for name in have.iter().filter(|name| !wanted.contains(name)) {
        let url = confluence.v1(&format!("/content/{id}/label"), &[("name", name.clone())]);
        confluence.change(ctx, Effect::Write, Method::Delete, &url, None)?;
    }
    Ok(())
}

command! {
    pub PAGE_UPDATE = ["confluence", "page", "update"], Write,
    "Edit a page: replace or append to its body or a section, retitle, move, relabel",
    keywords: ["edit", "change", "replace", "append", "section", "rename", "move", "label", "rollback"],
    example: "confluence page update 1101 --append-file rollback.md --message 'Add the rollback steps'",
    run: page_update,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{V1, V2, confluence, dry_run, page, piped, space};

    const BODY: &str = "<p>intro</p><h2>Steps</h2><p>Fix the order.</p><h2>Notes</h2><ac:structured-macro ac:name=\"jira\"><ac:parameter ac:name=\"key\">OPS-1</ac:parameter></ac:structured-macro>";

    fn read(version: u64) -> Vec<Answer> {
        vec![
            Answer::json(&page("1101", "Runbook", version, BODY)),
            Answer::json(&space()),
        ]
    }

    #[test]
    fn a_section_is_replaced_and_the_rest_is_kept_byte_for_byte() {
        let plans = dry_run(
            &[
                "confluence",
                "page",
                "update",
                "1101",
                "--section",
                "steps",
                "--body",
                "## Steps\n\nRetry **once**.",
                "--message",
                "retry",
            ],
            read(4),
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(plans[0]["url"], format!("{V2}/pages/1101"));
        assert_eq!(
            plans[0]["body"],
            json!({"id": "1101", "status": "current", "title": "Runbook",
                "body": {"representation": "storage", "value": BODY.replace("<h2>Steps</h2><p>Fix the order.</p>", "<h2>Steps</h2><p>Retry <strong>once</strong>.</p>")},
                "version": {"number": 5, "message": "retry"}})
        );
    }

    #[test]
    fn appending_to_a_section_goes_before_the_next_heading() {
        let (outcome, transport) = piped(
            &[
                "confluence",
                "page",
                "update",
                "1101",
                "--section",
                "Steps",
                "--append",
                "-",
            ],
            "Then tell @<557058:sam>.",
            [
                read(4),
                vec![Answer::json(
                    &json!({"id": "1101", "title": "Runbook", "version": {"number": 5}}),
                )],
            ]
            .concat(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["version"], 5);
        let sent = transport.sent();
        assert_eq!(
            sent[2].body.as_ref().unwrap()["body"]["value"],
            BODY.replace(
                "<h2>Notes</h2>",
                "<p>Then tell <ac:link><ri:user ri:account-id=\"557058:sam\" /></ac:link>.</p><h2>Notes</h2>"
            )
        );
    }

    #[test]
    fn a_whole_body_needs_if_version_and_the_loss_gate_guards_macros() {
        let (outcome, transport) = confluence(
            &["confluence", "page", "update", "1101", "--body", "x"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli confluence page get 1101 --fields version")
        );
        assert!(transport.sent().is_empty());

        let (outcome, _) = confluence(
            &[
                "confluence",
                "page",
                "update",
                "1101",
                "--body",
                "x",
                "--if-version",
                "3",
            ],
            read(4),
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome.stderr.contains("changed since version 3 (now 4)"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = confluence(
            &[
                "confluence",
                "page",
                "update",
                "1101",
                "--section",
                "Notes",
                "--body",
                "x",
            ],
            read(4),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("(jira)"), "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("agent-cli confluence page update 1101 --storage --body-file page.xml --if-version 4"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_409_naming_a_newer_version_is_a_conflict() {
        let (outcome, _) = confluence(
            &["confluence", "page", "update", "1101", "--append", "more"],
            [read(4), vec![Answer::status(409, r#"{"errors":[{"status":409,"code":"CONFLICT","title":"Version must be incremented when updating a page. Current Version: [6]. Provided version: [5]","detail":null}]}"#)]].concat(),
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("page 1101 changed since version 4 (now 6)"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_title_alone_a_move_and_labels_send_no_body() {
        let plans = dry_run(
            &[
                "confluence",
                "page",
                "update",
                "1101",
                "--title",
                "Runbook v2",
            ],
            read(4),
        );
        assert_eq!(plans[0]["url"], format!("{V2}/pages/1101/title"));
        assert_eq!(
            plans[0]["body"],
            json!({"status": "current", "title": "Runbook v2"})
        );

        let plans = dry_run(
            &["confluence", "page", "update", "1101", "--parent", "1200"],
            read(4),
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(
            plans[0]["url"],
            format!("{V1}/content/1101/move/append/1200")
        );
        assert!(plans[0].get("body").is_none());

        let plans = dry_run(
            &[
                "confluence",
                "page",
                "update",
                "1101",
                "--labels",
                "runbook,airflow",
            ],
            read(4),
        );
        assert_eq!(plans[0]["url"], format!("{V1}/content/1101/label"));
        assert_eq!(
            plans[0]["body"],
            json!([{"prefix": "global", "name": "airflow"}])
        );

        let (outcome, _) = confluence(
            &[
                "confluence",
                "page",
                "update",
                "1101",
                "--title",
                "x",
                "--parent",
                "1",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2);
    }
}
