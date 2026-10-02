use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::{API, Ado, Kind};

use super::{AttachmentRow, attachments};

#[derive(clap::Args)]
pub struct AttachmentListArgs {
    /// The work item: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn attachment_list(ctx: &Ctx, args: AttachmentListArgs) -> Result<Vec<AttachmentRow>> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let url = ado.api(
        None,
        &format!("wit/workitems/{id}"),
        "$expand=relations",
        API,
    );
    let mut rows = attachments(&ado.get(ctx, &url)?);
    if rows.len() > args.limit {
        ctx.note(format!("[{} of {}; --limit N]", args.limit, rows.len()));
        rows.truncate(args.limit);
    }
    Ok(rows)
}

command! {
    pub ATTACHMENT_LIST = ["ado", "attachment", "list"], Read,
    "List a work item's attachments: the files (logs, screenshots) attached to it",
    keywords: ["files", "attached", "screenshot", "upload", "document", "image", "log"],
    example: "ado attachment list 1207 --fields id,name,size",
    run: attachment_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::super::tests::{LOGO, SPEC, work_item};
    use crate::testing::{BASE, ado, urls};

    #[test]
    fn list_prints_the_attached_files_and_skips_other_links() {
        let (outcome, transport) = ado(
            &["ado", "attachment", "list", "AB#299"],
            vec![Answer::json(&work_item())],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": SPEC, "name": "Spec.txt", "size": 12, "date": "2026-09-29T20:49:26Z",
                 "comment": "Spec for the work"},
                {"id": LOGO, "name": "logo.png", "size": 2048, "date": "2026-09-30T08:00:00Z"},
            ])
        );
        assert_eq!(
            urls(&transport),
            [format!(
                "{BASE}/_apis/wit/workitems/299?$expand=relations&api-version=7.1"
            )]
        );
    }

    #[test]
    fn a_work_item_without_attachments_lists_none() {
        let (outcome, _) = ado(
            &["ado", "attachment", "list", "12"],
            vec![Answer::json(&json!({"id": 12, "rev": 1, "fields": {}}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!([]));
    }
}
