use agent_cli_core::{Ctx, command};
use anyhow::Result;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::client::{Confluence, PAGE_MAX, note_more};
use crate::ids::current_page;

use super::{AttachmentRow, EXPAND, row};

#[derive(clap::Args)]
pub struct AttachmentListArgs {
    /// The page: its id, KEY:Title or its URL
    page: String,
    /// Only files whose name holds this, any case
    #[arg(long)]
    name: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// ponytail: newest first means reading every file on the page, up to
/// 1,000; the list has no order of its own to ask for.
const MOST: usize = 1000;

fn attachment_list(ctx: &Ctx, args: AttachmentListArgs) -> Result<Vec<AttachmentRow>> {
    let confluence = Confluence::load(ctx)?;
    let id = current_page(ctx, &confluence, &args.page)?;
    // The page is read first so a wrong id is exit 4 with its hint.
    confluence.content(ctx, &id, "", None)?;
    let url = confluence.api(
        &format!("/content/{id}/child/attachment"),
        &[
            ("expand", EXPAND.to_owned()),
            ("limit", PAGE_MAX.to_string()),
        ],
    );
    let (mut found, cut) = confluence.list(ctx, url, MOST)?;
    let wanted = args.name.map(|name| name.trim().to_lowercase());
    found.retain(|file| {
        wanted.as_deref().is_none_or(|wanted| {
            file["title"]
                .as_str()
                .is_some_and(|name| name.to_lowercase().contains(wanted))
        })
    });
    found.sort_by_key(|file| {
        std::cmp::Reverse(
            file["version"]["when"]
                .as_str()
                .and_then(|when| OffsetDateTime::parse(when, &Rfc3339).ok()),
        )
    });
    let mut rows: Vec<AttachmentRow> = found.iter().filter_map(row).collect();
    let more = rows.len() > args.limit || cut;
    rows.truncate(args.limit);
    note_more(ctx, rows.len(), None, more);
    Ok(rows)
}

command! {
    pub ATTACHMENT_LIST = ["confluence", "attachment", "list"], Read,
    "List the files attached to a page, newest first",
    keywords: ["files", "attached", "uploads", "documents", "images", "logs", "csv"],
    example: "confluence attachment list 1201 --fields id,name,size",
    run: attachment_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{API, SAM, confluence, page, urls, user};

    #[test]
    fn a_pages_files_list_newest_first_with_their_ids_sizes_and_authors() {
        let file = |id: &str, name: &str, media: &str, size: u64, when: &str| {
            json!({"id": id, "type": "attachment", "status": "current", "title": name,
                "version": {"by": user(SAM, "sam", "Sam Lee"), "when": when, "number": 1},
                "container": {"id": "1201", "type": "page"},
                "extensions": {"mediaType": media, "fileSize": size, "comment": ""},
                "_links": {"download": format!("/download/attachments/1201/{name}?version=1&api=v2")}})
        };
        let files = json!({"results": [
            file("7002", "rollout.png", "image/png", 20480, "2026-09-28T16:31:00.000Z"),
            file("7001", "changes-v1.4.2.txt", "text/plain", 412, "2026-09-28T16:30:00.000Z"),
            file("7003", "changes-v1.4.1.txt", "text/plain", 300, "2026-09-20T10:00:00.000Z")],
            "start": 0, "limit": 200, "size": 3, "_links": {}});
        let (outcome, transport) = confluence(
            &[
                "confluence",
                "attachment",
                "list",
                "1201",
                "--name",
                "CHANGES",
            ],
            vec![
                Answer::json(&page("1201", "Release notes", 3, "")),
                Answer::json(&files),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "7001", "page": "1201", "name": "changes-v1.4.2.txt", "media_type": "text/plain",
                "size": 412, "version": 1, "created": "2026-09-28T16:30:00Z", "author": "Sam Lee"},
                {"id": "7003", "page": "1201", "name": "changes-v1.4.1.txt", "media_type": "text/plain",
                "size": 300, "version": 1, "created": "2026-09-20T10:00:00Z", "author": "Sam Lee"}])
        );
        assert_eq!(
            urls(&transport)[1],
            format!("{API}/content/1201/child/attachment?expand=version%2Ccontainer&limit=200")
        );
    }
}
