use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Failure, Method, command, status_of};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::{Confluence, PAGE, text};
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
    /// Move the page under this page id, in the same space (the REST API moves no page to another)
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
    let content = confluence.content(ctx, &id, PAGE, None)?;
    let version = content.version();
    if let Some(wanted) = args.if_version
        && wanted != version
    {
        return Err(changed(&id, wanted, version));
    }
    let key = content.space().unwrap_or_default();
    let url = confluence.api(&format!("/content/{id}"), &[]);
    let title = args.title.clone().or_else(|| text(&content.value["title"]));
    let mut written = Written {
        id: id.clone(),
        title: title.clone(),
        space: Some(key.clone()),
        version: Some(version),
        url: confluence.web_of(&content.value),
    };
    let mut request = json!({
        "id": id, "type": content.kind, "title": title,
        "version": {"number": version + 1},
    });
    if let Some(parent) = args.parent {
        let parent = parent.to_string();
        let there = confluence
            .content(ctx, &parent, "space", None)?
            .space()
            .unwrap_or_default();
        if !there.eq_ignore_ascii_case(&key) {
            return Err(Failure::usage(format!(
                "page {parent} is in space {there} and page {id} in {key}: the REST API moves a page only within its space"
            ))
            .hint(format!(
                "move it in the browser (\u{2026} menu, Move): {}",
                written.url.clone().unwrap_or_default()
            ))
            .into());
        }
        request["ancestors"] = json!([{"id": parent}]);
    } else if let Some(wanted) = args.labels.as_deref().map(labels) {
        relabel(ctx, &confluence, &id, &content.labels(), &wanted)?;
        return Ok(written);
    } else if body.is_some() || append.is_some() {
        let old = content.storage().to_owned();
        let text = body.as_ref().or(append.as_ref()).map(|text| &text.text);
        let text = text.cloned().unwrap_or_default();
        let nodes = parse(&old);
        let range = match &args.section {
            Some(name) => find(&sections(&old, &nodes), name, &id)?.range.clone(),
            None => 0..old.len(),
        };
        let part = if args.storage {
            text
        } else {
            let people = people(ctx, &confluence, &text, &mentions(&nodes))?;
            to_storage(
                &text,
                &Target {
                    space: &key,
                    people: &people,
                },
            )
        };
        let new = if body.is_some() {
            if !args.storage {
                gate(&old[range.clone()], &id, version, args.section.is_some())?;
            }
            format!("{}{part}{}", &old[..range.start], &old[range.end..])
        } else {
            format!("{}{part}{}", &old[..range.end], &old[range.end..])
        };
        request["body"] = json!({"storage": {"value": new, "representation": "storage"}});
        if let Some(message) = &args.message {
            request["version"]["message"] = json!(message);
        }
    }
    // A title or a move alone sends no body, so nothing read can overwrite
    // a newer one; the version asserted refuses any change made since.
    let answer = confluence
        .change(ctx, Effect::Write, Method::Put, &url, Some(request))
        .map_err(|error| conflict(error, &id, version))?;
    written.title = text(&answer["title"]).or(written.title);
    written.version = answer["version"]["number"].as_u64().or(Some(version + 1));
    written.url = confluence.web_of(&answer).or(written.url);
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
/// any other 409 keeps the service's words.
fn conflict(error: anyhow::Error, id: &str, read: u64) -> anyhow::Error {
    if status_of(&error) != Some(409) {
        return error;
    }
    let said = format!("{error:#}");
    match said
        .split_once("Current version is: ")
        .map(|(_, rest)| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|now| now.parse::<u64>().ok())
    {
        Some(now) => changed(id, read, now),
        None => error,
    }
}

/// Labels as asked: the missing ones added in one call, the rest removed
/// one by one. Labels make no new version.
fn relabel(
    ctx: &Ctx,
    confluence: &Confluence,
    id: &str,
    have: &[String],
    wanted: &[String],
) -> Result<()> {
    let add: Vec<Value> = wanted
        .iter()
        .filter(|name| !have.contains(name))
        .map(|name| json!({"prefix": "global", "name": name}))
        .collect();
    if !add.is_empty() {
        let url = confluence.api(&format!("/content/{id}/label"), &[]);
        confluence.change(
            ctx,
            Effect::Write,
            Method::Post,
            &url,
            Some(Value::Array(add)),
        )?;
    }
    for name in have.iter().filter(|name| !wanted.contains(name)) {
        let url = confluence.api(&format!("/content/{id}/label"), &[("name", name.clone())]);
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

    use crate::testing::{API, BASE, confluence, dry_run, page, piped, user};

    const BODY: &str = "<p>intro</p><h2>Steps</h2><p>Fix the order.</p><h2>Notes</h2><ac:structured-macro ac:name=\"jira\"><ac:parameter ac:name=\"key\">OPS-1</ac:parameter></ac:structured-macro>";

    fn read(version: u64) -> Vec<Answer> {
        vec![Answer::json(&page("1101", "Runbook", version, BODY))]
    }

    fn put(version: u64) -> Answer {
        Answer::json(&json!({"id": "1101", "type": "page", "title": "Runbook",
            "version": {"number": version}, "_links": {"webui": "/spaces/ENG/pages/1101/Runbook", "base": BASE}}))
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
        assert_eq!(plans[0]["url"], format!("{API}/content/1101"));
        assert_eq!(
            plans[0]["body"],
            json!({"id": "1101", "type": "page", "title": "Runbook",
                "body": {"storage": {"value": BODY.replace("<h2>Steps</h2><p>Fix the order.</p>", "<h2>Steps</h2><p>Retry <strong>once</strong>.</p>"), "representation": "storage"}},
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
            "Then tell @<sam>.",
            [
                read(4),
                vec![
                    Answer::json(&json!({"results": [], "totalSize": 0})),
                    Answer::json(&user("8a80sam", "sam", "Sam Lee")),
                    put(5),
                ],
            ]
            .concat(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["version"], 5);
        let sent = transport.sent();
        assert!(
            sent[1].url.starts_with(&format!(
                "{API}/search?cql=type+%3D+user+and+user.fullname+~+%22sam%22"
            )),
            "{}",
            sent[1].url
        );
        assert_eq!(sent[2].url, format!("{API}/user?username=sam"));
        assert_eq!(
            sent[3].body.as_ref().unwrap()["body"]["storage"]["value"],
            BODY.replace(
                "<h2>Notes</h2>",
                "<p>Then tell <ac:link><ri:user ri:userkey=\"8a80sam\" /></ac:link>.</p><h2>Notes</h2>"
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
            [read(4), vec![Answer::status(409, r#"{"statusCode":409,"data":{"authorized":false,"valid":true,"allowedInReadOnlyMode":true,"errors":[],"successful":false},"message":"Version must be incremented on update. Current version is: 6. Log referral number is 00000000-0000-4000-8000-000000000002","reason":"Conflict"}"#)]].concat(),
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
        assert_eq!(plans[0]["url"], format!("{API}/content/1101"));
        assert_eq!(
            plans[0]["body"],
            json!({"id": "1101", "type": "page", "title": "Runbook v2", "version": {"number": 5}})
        );

        let plans = dry_run(
            &["confluence", "page", "update", "1101", "--parent", "1200"],
            [
                read(4),
                vec![Answer::json(&page("1200", "Release notes", 2, ""))],
            ]
            .concat(),
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(
            plans[0]["body"],
            json!({"id": "1101", "type": "page", "title": "Runbook", "version": {"number": 5},
                "ancestors": [{"id": "1200"}]})
        );

        let mut elsewhere = page("3001", "Pager", 1, "");
        elsewhere["space"] = json!({"key": "OPS"});
        let (outcome, transport) = confluence(
            &["confluence", "page", "update", "1101", "--parent", "3001"],
            [read(4), vec![Answer::json(&elsewhere)]].concat(),
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("moves a page only within its space"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().iter().all(|sent| sent.method.is_read()));

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
        assert_eq!(plans[0]["url"], format!("{API}/content/1101/label"));
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
