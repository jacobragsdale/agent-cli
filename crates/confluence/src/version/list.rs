use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Confluence, PAGE_MAX, note_more, stamp, text};
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

fn version_list(ctx: &Ctx, args: VersionListArgs) -> Result<Vec<VersionRow>> {
    let confluence = Confluence::load(ctx)?;
    let id = current_page(ctx, &confluence, &args.page)?;
    let content = confluence.content(ctx, &id, &[])?;
    let url = confluence.v2(
        &format!("/{}/{id}/versions", content.kind),
        &[
            ("sort", "-modified-date".to_owned()),
            ("limit", args.limit.clamp(1, PAGE_MAX).to_string()),
        ],
    );
    let (found, more) = confluence.list(ctx, url, args.limit)?;
    let authors: Vec<String> = found
        .iter()
        .filter_map(|version| text(&version["authorId"]))
        .collect();
    let names = confluence.names(ctx, &authors);
    let rows: Vec<VersionRow> = found
        .iter()
        .filter_map(|version| {
            let number = version["number"].as_u64()?;
            Some(VersionRow {
                id: format!("{id}@{number}"),
                version: number,
                author: text(&version["authorId"]).map(|id| names.get(&id).cloned().unwrap_or(id)),
                date: stamp(&version["createdAt"]),
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

    use crate::testing::{JANE, SAM, V2, confluence, page, people, urls};

    #[test]
    fn versions_list_newest_first_as_refs_page_get_takes() {
        let versions = json!({"results": [
            {"number": 3, "authorId": JANE, "createdAt": "2026-09-28T16:20:00.000Z", "message": "v1.4.2 shipped", "minorEdit": false},
            {"number": 2, "authorId": SAM, "createdAt": "2026-09-27T10:00:00.000Z", "message": "", "minorEdit": true}],
            "_links": {}});
        let (outcome, transport) = confluence(
            &["confluence", "version", "list", "1201"],
            vec![
                Answer::json(&page("1201", "Release notes", 3, "")),
                Answer::json(&versions),
                people(),
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
            format!("{V2}/pages/1201/versions?sort=-modified-date&limit=50")
        );
    }
}
