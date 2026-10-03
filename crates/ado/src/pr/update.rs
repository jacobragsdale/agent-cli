use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::{Ado, Kind};

use super::{PrRow, active_pr, pr_row};

#[derive(Clone, Copy, clap::ValueEnum)]
enum Toggle {
    On,
    Off,
}

#[derive(clap::Args)]
pub struct PrUpdateArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// Complete it by itself once policies pass, as pr complete does by
    /// default: squash, delete the source branch, transition the work items
    #[arg(long, value_enum)]
    autocomplete: Option<Toggle>,
    /// True to make it a draft, false to publish it
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    draft: Option<bool>,
    /// A new title
    #[arg(long)]
    title: Option<String>,
    /// Markdown, replacing the description; - reads stdin
    #[arg(long, allow_hyphen_values = true)]
    description: Option<String>,
    /// The description from a Markdown file
    #[arg(long)]
    description_file: Option<PathBuf>,
}

fn pr_update(ctx: &Ctx, args: PrUpdateArgs) -> Result<PrRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let mut body = serde_json::Map::new();
    if let Some(draft) = args.draft {
        body.insert("isDraft".into(), draft.into());
    }
    if let Some(title) = &args.title {
        body.insert("title".into(), title.trim().into());
    }
    if let Some(description) = ctx.long_text(
        "description",
        args.description.as_deref(),
        args.description_file.as_deref(),
        None,
    )? {
        body.insert("description".into(), description.text.into());
    }
    if body.is_empty() && args.autocomplete.is_none() {
        return Err(Failure::usage("nothing to change")
            .hint(format!("pass --autocomplete on|off, --draft true|false, --title, --description or --description-file, e.g. agent-cli ado pr update {} --autocomplete on", id))
            .into());
    }
    // Azure DevOps refuses an edit to a closed pull request (TF401181).
    let (_, repo_id) = active_pr(ctx, &ado, id)?;
    match args.autocomplete {
        Some(Toggle::On) => {
            body.insert("autoCompleteSetBy".into(), json!({"id": ado.me(ctx)?.id}));
            // Without these Azure DevOps merges with a merge commit, keeps the
            // branch and leaves the work items: not what pr complete does.
            body.insert(
                "completionOptions".into(),
                json!({"mergeStrategy": "squash", "deleteSourceBranch": true, "transitionWorkItems": true}),
            );
        }
        Some(Toggle::Off) => {
            // The empty GUID is how the API is told nobody set it.
            body.insert(
                "autoCompleteSetBy".into(),
                json!({"id": "00000000-0000-0000-0000-000000000000"}),
            );
        }
        None => {}
    }
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullrequests/{}", id),
        "",
    );
    let updated = ado.change(ctx, Effect::Write, Method::Patch, &url, Value::Object(body))?;
    Ok(pr_row(&ado, &updated))
}

command! {
    pub PR_UPDATE = ["ado", "pr", "update"], Write,
    "Turn auto-complete on or off, mark draft or ready, or retitle a pull request",
    keywords: ["autocomplete", "auto", "complete", "draft", "publish", "ready", "rename", "edit"],
    example: "ado pr update 42 --autocomplete on",
    run: pr_update,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, dry_run, me, pr};

    #[test]
    fn update_sets_auto_complete_as_you_and_publishes_a_draft_in_one_patch() {
        let plans = dry_run(
            &[
                "ado",
                "pr",
                "update",
                "17",
                "--autocomplete",
                "on",
                "--draft",
                "false",
            ],
            vec![Answer::json(&pr(17, true)), me()],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/git/repositories/r-1/pullrequests/17?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"isDraft": false, "autoCompleteSetBy": {"id": "u-1"}, "completionOptions":
                {"mergeStrategy": "squash", "deleteSourceBranch": true, "transitionWorkItems": true}})
        );

        let plans = dry_run(
            &["ado", "pr", "update", "17", "--autocomplete", "off"],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(
            plans[0]["body"],
            json!({"autoCompleteSetBy": {"id": "00000000-0000-0000-0000-000000000000"}})
        );

        let (outcome, transport) = ado(&["ado", "pr", "update", "17"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }
}
