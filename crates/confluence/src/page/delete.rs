use agent_cli_core::{Ctx, Effect, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Confluence, text};
use crate::ids::current_page;

#[derive(clap::Args)]
pub struct PageDeleteArgs {
    /// The page: its id, KEY:Title or its URL
    page: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Trashed {
    id: String,
    title: Option<String>,
    /// trashed: the space's trash restores it.
    status: String,
}

/// The trash, never a purge: a space admin restores it from there.
fn page_delete(ctx: &Ctx, args: PageDeleteArgs) -> Result<Trashed> {
    let confluence = Confluence::load(ctx)?;
    let id = current_page(ctx, &confluence, &args.page)?;
    let content = confluence.content(ctx, &id, &[])?;
    let url = confluence.v2(&format!("/{}/{id}", content.kind), &[]);
    confluence.change(ctx, Effect::Destructive, Method::Delete, &url, None)?;
    Ok(Trashed {
        title: text(&content.value["title"]),
        id,
        status: "trashed".to_owned(),
    })
}

command! {
    pub PAGE_DELETE = ["confluence", "page", "delete"], Destructive,
    "Move a page to its space's trash, from which it can be restored",
    keywords: ["remove", "trash", "archive", "drop"],
    example: "confluence page delete 1102 --yes",
    run: page_delete,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{V2, confluence, dry_run, page};

    #[test]
    fn a_page_goes_to_the_trash_with_yes() {
        let plans = dry_run(
            &["confluence", "page", "delete", "1102"],
            vec![Answer::json(&page("1102", "Old runbook", 2, ""))],
        );
        assert_eq!(plans[0]["method"], "DELETE");
        assert_eq!(plans[0]["url"], format!("{V2}/pages/1102"));
        let (outcome, _) = confluence(&["confluence", "page", "delete", "1102"], vec![]);
        assert_eq!(outcome.code, 2, "a delete needs --yes: {outcome:?}");
        let (outcome, _) = confluence(
            &["confluence", "page", "delete", "1102", "--yes"],
            vec![
                Answer::json(&page("1102", "Old runbook", 2, "")),
                Answer::status(204, ""),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "1102", "title": "Old runbook", "status": "trashed"})
        );
    }
}
