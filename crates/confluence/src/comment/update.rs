use agent_cli_core::{Ctx, Effect, Failure, Method, command, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Confluence, INLINE, text};
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

/// Only inline comments resolve, a thread at a time through its first
/// comment. The plugin would mark a footer comment too, so the comment is
/// read first.
fn comment_update(ctx: &Ctx, args: CommentUpdateArgs) -> Result<Resolved> {
    let confluence = Confluence::load(ctx)?;
    let id = comment(&confluence.site, &args.comment)?;
    let url = confluence.api(
        &format!("/content/{id}"),
        &[("expand", "container,ancestors".to_owned())],
    );
    let missing = || -> anyhow::Error {
        Failure::not_found(format!("no comment {id}, or you may not see it"))
            .hint("agent-cli confluence comment list PAGE --kind inline")
            .into()
    };
    let found = match confluence.get(ctx, &url) {
        Ok(found) if found["type"] == "comment" => found,
        Ok(_) => return Err(missing()),
        Err(error) if status_of(&error) == Some(404) => return Err(missing()),
        Err(error) => return Err(error),
    };
    let page = text(&found["container"]["id"]);
    let on = page.clone().unwrap_or_else(|| "PAGE".to_owned());
    if found["extensions"]["location"] != "inline" {
        return Err(Failure::usage(format!(
            "comment {id} is a footer comment: only inline comments resolve"
        ))
        .hint(format!(
            "agent-cli confluence comment list {on} --kind inline"
        ))
        .into());
    }
    let root = found["ancestors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|ancestor| ancestor["type"] == "comment")
        .filter_map(|ancestor| text(&ancestor["id"]))
        .next_back();
    if let Some(root) = root {
        return Err(Failure::usage(format!(
            "comment {id} is a reply, and its thread resolves as one, by comment {root}"
        ))
        .hint(format!(
            "agent-cli confluence comment update {root} --status {}",
            if args.status == Status::Resolved {
                "resolved"
            } else {
                "open"
            }
        ))
        .into());
    }
    let resolved = args.status == Status::Resolved;
    let url = confluence.url(
        INLINE,
        &format!("/comments/{id}/resolve/{resolved}/dangling/false"),
        &[],
    );
    let answer = confluence.change(ctx, Effect::Write, Method::Put, &url, None)?;
    let now = answer["resolveProperties"]["resolved"]
        .as_bool()
        .unwrap_or(resolved);
    Ok(Resolved {
        id,
        page,
        resolution: Some(if now { "resolved" } else { "reopened" }.to_owned()),
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

    use crate::testing::{API, confluence, dry_run};

    fn found(location: &str, ancestors: serde_json::Value) -> Answer {
        Answer::json(
            &json!({"id": "5002", "type": "comment", "status": "current",
            "container": {"id": "1101", "type": "page"}, "ancestors": ancestors,
            "extensions": {"location": location}}),
        )
    }

    #[test]
    fn resolving_goes_through_the_inline_comments_plugin() {
        let plans = dry_run(
            &[
                "confluence",
                "comment",
                "update",
                "5002",
                "--status",
                "resolved",
            ],
            vec![found("inline", json!([]))],
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(
            plans[0]["url"],
            "https://wiki.contoso.example/confluence/rest/inlinecomments/1.0/comments/5002/resolve/true/dangling/false"
        );
        let (outcome, transport) = confluence(
            &[
                "confluence",
                "comment",
                "update",
                "5002",
                "--status",
                "open",
            ],
            vec![
                found("inline", json!([])),
                Answer::json(&json!({"resolveProperties": {"resolved": false, "resolvedTime": 0}})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "5002", "page": "1101", "resolution": "reopened"})
        );
        assert_eq!(
            transport.sent()[0].url,
            format!("{API}/content/5002?expand=container%2Cancestors")
        );
    }

    #[test]
    fn a_footer_comment_or_a_reply_does_not_resolve() {
        let (outcome, transport) = confluence(
            &[
                "confluence",
                "comment",
                "update",
                "5002",
                "--status",
                "resolved",
            ],
            vec![found("footer", json!([]))],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("only inline comments resolve"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1);
        let (outcome, _) = confluence(
            &[
                "confluence",
                "comment",
                "update",
                "5002",
                "--status",
                "resolved",
            ],
            vec![found("inline", json!([{"id": "5001", "type": "comment"}]))],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli confluence comment update 5001 --status resolved"),
            "{}",
            outcome.stderr
        );
    }
}
