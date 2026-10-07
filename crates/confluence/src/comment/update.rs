use agent_cli_core::{Ctx, Effect, Failure, Method, command, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Confluence, text};
use crate::ids::comment;

#[derive(clap::Args)]
pub struct CommentUpdateArgs {
    /// The inline comment: its id, or a URL with focusedCommentId
    comment: String,
    /// resolved, or open to reopen it
    #[arg(long, value_enum)]
    status: Status,
}

#[derive(Clone, Copy, PartialEq, clap::ValueEnum)]
pub enum Status {
    Resolved,
    Open,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Resolved {
    id: String,
    page: Option<String>,
    /// open, reopened or resolved.
    resolution: Option<String>,
}

/// Only inline comments resolve. The body is sent back as it was read, so
/// the call changes the resolution and nothing else.
fn comment_update(ctx: &Ctx, args: CommentUpdateArgs) -> Result<Resolved> {
    let confluence = Confluence::load(ctx)?;
    let id = comment(&confluence.site, &args.comment)?;
    let url = confluence.v2(&format!("/inline-comments/{id}"), &[]);
    let read = confluence.get(ctx, &format!("{url}?body-format=storage"));
    let found = match read {
        Ok(found) => found,
        Err(error) if status_of(&error) == Some(404) => {
            return Err(Failure::not_found(format!(
                "no inline comment {id}: only inline comments resolve, and a footer comment is not one"
            ))
            .hint("agent-cli confluence comment list PAGE --kind inline")
            .into());
        }
        Err(error) => return Err(error),
    };
    let page = text(&found["pageId"]).or_else(|| text(&found["blogPostId"]));
    let resolved = args.status == Status::Resolved;
    let version = found["version"]["number"].as_u64().unwrap_or(1);
    let body = found["body"]["storage"]["value"]
        .as_str()
        .unwrap_or_default();
    let request = json!({
        "version": {"number": version + 1},
        "body": {"representation": "storage", "value": body},
        "resolved": resolved,
    });
    let answer = confluence.change(ctx, Effect::Write, Method::Put, &url, Some(request))?;
    Ok(Resolved {
        id,
        page,
        resolution: text(&answer["resolutionStatus"])
            .or_else(|| Some(if resolved { "resolved" } else { "reopened" }.to_owned())),
    })
}

command! {
    pub COMMENT_UPDATE = ["confluence", "comment", "update"], Write,
    "Resolve an inline comment on a page, or reopen it",
    keywords: ["resolve", "reopen", "close", "done", "inline", "thread", "answered"],
    example: "confluence comment update 5002 --status resolved",
    run: comment_update,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{V2, confluence, dry_run};

    #[test]
    fn resolving_bumps_the_comments_version_and_keeps_its_body() {
        let found = json!({"id": "5002", "pageId": "1101", "resolutionStatus": "open",
            "version": {"number": 1}, "body": {"storage": {"value": "<p>only once</p>"}}});
        let plans = dry_run(
            &[
                "confluence",
                "comment",
                "update",
                "5002",
                "--status",
                "resolved",
            ],
            vec![Answer::json(&found)],
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(plans[0]["url"], format!("{V2}/inline-comments/5002"));
        assert_eq!(
            plans[0]["body"],
            json!({"version": {"number": 2}, "body": {"representation": "storage", "value": "<p>only once</p>"}, "resolved": true})
        );
        let (outcome, _) = confluence(
            &[
                "confluence",
                "comment",
                "update",
                "5001",
                "--status",
                "resolved",
            ],
            vec![Answer::status(
                404,
                r#"{"errors":[{"status":404,"title":"Not found"}]}"#,
            )],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("only inline comments resolve"),
            "{}",
            outcome.stderr
        );
    }
}
