use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Confluence, EXPERIMENTAL, PAGE_MAX, note_more, person, stamp, text};
use crate::ids::current_page;

#[derive(clap::Args)]
pub struct VersionListArgs {
    /// The page: its id, KEY:Title or its URL
    page: String,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct VersionRow {
    /// ID@VERSION: what page get takes for that version's body.
    id: String,
    version: u64,
    author: Option<String>,
    date: Option<String>,
    /// The version comment.
    message: Option<String>,
    /// A minor edit, which notified no one.
    minor: bool,
}

/// Newest first, from `/rest/experimental`: the stable `/rest/api` has no
/// version list on Data Center (9.2 answers it 404).
fn version_list(ctx: &Ctx, args: VersionListArgs) -> Result<Vec<VersionRow>> {
    let confluence = Confluence::load(ctx)?;
    let id = current_page(ctx, &confluence, &args.page)?;
    // The page is read first so a wrong id is exit 4 with its hint.
    confluence.content(ctx, &id, "", None)?;
    let url = confluence.url(
        EXPERIMENTAL,
        &format!("/content/{id}/version"),
        &[("limit", args.limit.clamp(1, PAGE_MAX).to_string())],
    );
    let (found, more) = confluence.list(ctx, url, args.limit)?;
    let rows: Vec<VersionRow> = found
        .iter()
        .filter_map(|version| {
            let number = version["number"].as_u64()?;
            Some(VersionRow {
                id: format!("{id}@{number}"),
                version: number,
                author: person(&version["by"]),
                date: stamp(&version["when"]),
                message: text(&version["message"]),
                minor: version["minorEdit"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    note_more(ctx, rows.len(), None, more);
    Ok(rows)
}

command! {
    pub VERSION_LIST = ["confluence", "version", "list"], Read,
    "List a page's versions, newest first: who changed it, when and why",
    keywords: ["history", "revisions", "changes", "edited", "who", "when", "changelog", "diff"],
    example: "confluence version list 1201 --limit 5 --fields id,author,date,message",
    run: version_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, JANE, SAM, confluence, page, urls, user};

    #[test]
    fn versions_list_newest_first_as_refs_page_get_takes() {
        let versions = json!({"results": [
            {"by": user(JANE, "jane", "Jane Doe"), "when": "2026-09-28T16:20:00.000Z", "message": "v1.4.2 shipped", "number": 3, "minorEdit": false, "hidden": false},
            {"by": user(SAM, "sam", "Sam Lee"), "when": "2026-09-27T10:00:00.000Z", "message": "", "number": 2, "minorEdit": true, "hidden": false}],
            "start": 0, "limit": 50, "size": 2, "_links": {"base": BASE}});
        let (outcome, transport) = confluence(
            &["confluence", "version", "list", "1201"],
            vec![
                Answer::json(&page("1201", "Release notes", 3, "")),
                Answer::json(&versions),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "1201@3", "version": 3, "author": "Jane Doe", "date": "2026-09-28T16:20:00Z",
                    "message": "v1.4.2 shipped", "minor": false},
                {"id": "1201@2", "version": 2, "author": "Sam Lee", "date": "2026-09-27T10:00:00Z", "minor": true}])
        );
        assert_eq!(
            urls(&transport)[1],
            format!("{BASE}/rest/experimental/content/1201/version?limit=50")
        );
    }
}
