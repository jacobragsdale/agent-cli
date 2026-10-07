use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::{Confluence, PAGE_MAX, note_more, text};
use crate::ids::current_page;

use super::{AttachmentRow, row};

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

fn attachment_list(ctx: &Ctx, args: AttachmentListArgs) -> Result<Vec<AttachmentRow>> {
    let confluence = Confluence::load(ctx)?;
    let id = current_page(ctx, &confluence, &args.page)?;
    let content = confluence.content(ctx, &id, &[])?;
    let url = confluence.v2(
        &format!("/{}/{id}/attachments", content.kind),
        &[
            ("sort", "-created-date".to_owned()),
            ("limit", PAGE_MAX.to_string()),
        ],
    );
    let wanted = args.name.map(|name| name.trim().to_lowercase());
    let fetch = if wanted.is_some() {
        usize::MAX
    } else {
        args.limit
    };
    let (found, more) = confluence.list(ctx, url, fetch)?;
    let found: Vec<_> = found
        .into_iter()
        .filter(|file| {
            wanted.as_deref().is_none_or(|wanted| {
                file["title"]
                    .as_str()
                    .is_some_and(|name| name.to_lowercase().contains(wanted))
            })
        })
        .collect();
    let authors: Vec<String> = found
        .iter()
        .filter_map(|file| text(&file["version"]["authorId"]))
        .collect();
    let names = confluence.names(ctx, &authors);
    let mut rows: Vec<AttachmentRow> = found.iter().filter_map(|file| row(file, &names)).collect();
    let cut = rows.len() > args.limit;
    rows.truncate(args.limit);
    note_more(ctx, rows.len(), None, cut || more);
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

    use crate::testing::{SAM, V2, confluence, page, people, urls};

    #[test]
    fn a_pages_files_list_with_their_ids_sizes_and_authors() {
        let files = json!({"results": [
            {"id": "att7001", "title": "changes-v1.4.2.txt", "pageId": "1201", "mediaType": "text/plain",
                "fileSize": 412, "comment": "", "createdAt": "2026-09-28T16:30:00.000Z",
                "version": {"number": 1, "authorId": SAM}},
            {"id": "att7002", "title": "rollout.png", "pageId": "1201", "mediaType": "image/png",
                "fileSize": 20480, "createdAt": "2026-09-28T16:31:00.000Z", "version": {"number": 2, "authorId": SAM}}],
            "_links": {}});
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
                people(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "att7001", "page": "1201", "name": "changes-v1.4.2.txt", "media_type": "text/plain",
                "size": 412, "version": 1, "created": "2026-09-28T16:30:00Z", "author": "Sam Lee"}])
        );
        assert_eq!(
            urls(&transport)[1],
            format!("{V2}/pages/1201/attachments?sort=-created-date&limit=250")
        );
    }
}
