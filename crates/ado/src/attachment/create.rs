use std::path::Path;

use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{API, Ado, Body, Kind, segment, text};

use super::{AttachmentRow, attachments};

#[derive(clap::Args)]
pub struct AttachmentCreateArgs {
    /// The work item: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// The file to attach (60 MB at most)
    #[arg(long)]
    file: String,
    /// A note shown beside the attachment
    #[arg(long)]
    comment: Option<String>,
}

/// Azure DevOps's own cap on one upload without chunking.
const MAX_BYTES: u64 = 60 * 1024 * 1024;

fn attachment_create(ctx: &Ctx, args: AttachmentCreateArgs) -> Result<AttachmentRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let path = Path::new(&args.file);
    let unreadable =
        |error: std::io::Error| Failure::usage(format!("cannot read {}: {error}", args.file));
    let size = std::fs::metadata(path).map_err(unreadable)?.len();
    if size > MAX_BYTES {
        return Err(Failure::usage(format!(
            "{} is {size} bytes, and Azure DevOps takes attachments up to 60 MB",
            args.file
        ))
        .into());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Failure::usage(format!("{} names no file", args.file)))?;
    let bytes = std::fs::read(path).map_err(unreadable)?;
    // Bytes uploaded for a work item that is not there would stay in the
    // organization with nothing pointing at them, so it is read first; its
    // project is where they are uploaded.
    let item = ado.get(
        ctx,
        &ado.api(
            None,
            &format!("wit/workitems/{id}"),
            "fields=System.TeamProject",
            API,
        ),
    )?;
    let project = text(&item["fields"]["System.TeamProject"]);
    // The bytes go first: the relation needs the URL the upload answers with.
    // %20, not +: the name is what the work item shows.
    let upload = ado.api(
        Some(project.as_deref().unwrap_or(&ado.project)),
        "wit/attachments",
        &format!("fileName={}", segment(name)),
        API,
    );
    let uploaded = ado
        .send(
            ctx,
            Method::Post,
            &upload,
            Body::Bytes(bytes),
            Some(Effect::Write),
        )?
        .json()?;
    let url = text(&uploaded["url"]).context("the upload came back without its URL")?;
    let mut relation = json!({"rel": "AttachedFile", "url": url});
    if let Some(comment) = &args.comment {
        relation["attributes"] = json!({ "comment": comment });
    }
    let item = ado.patch_work_item(
        ctx,
        Method::Patch,
        &ado.api(None, &format!("wit/workitems/{id}"), "", API),
        vec![json!({"op": "add", "path": "/relations/-", "value": relation})],
    )?;
    let guid = super::guid_in(&url).context("the upload answered a URL without a GUID")?;
    attachments(&item)
        .into_iter()
        .find(|row| row.id == guid)
        .context("the work item came back without the new attachment")
}

command! {
    pub ATTACHMENT_CREATE = ["ado", "attachment", "create"], Write,
    "Attach a file (a log, a screenshot) to a work item",
    keywords: ["upload", "add", "attach", "file", "screenshot", "document", "image"],
    example: "ado attachment create 1207 --file crash.log --comment 'Log from the failed run'",
    run: attachment_create,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::super::tests::{SPEC, work_item};
    use crate::testing::{BASE, ado, dry_run, urls};

    fn spec() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Spec v2.txt");
        std::fs::write(&path, "Spec for 299").unwrap();
        let path = path.to_str().unwrap().to_owned();
        (dir, path)
    }

    #[test]
    fn create_uploads_the_bytes_then_links_them_with_the_comment() {
        let (_dir, path) = spec();
        let uploaded = Answer::json(&json!({"id": SPEC,
            "url": format!("https://dev.azure.com/contoso/_apis/wit/attachments/{SPEC}?fileName=Spec%20v2.txt")}));
        let (outcome, transport) = ado(
            &[
                "ado",
                "attachment",
                "create",
                "299",
                "--file",
                &path,
                "--comment",
                "Spec for the work",
            ],
            vec![
                Answer::json(
                    &json!({"id": 299, "fields": {"System.TeamProject": "Contoso Mobile"}}),
                ),
                uploaded,
                Answer::json(&work_item()),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": SPEC, "name": "Spec.txt", "size": 12,
                "date": "2026-09-29T20:49:26Z", "comment": "Spec for the work"})
        );
        let sent = transport.sent();
        assert_eq!(
            urls(&transport),
            [
                format!("{BASE}/_apis/wit/workitems/299?fields=System.TeamProject&api-version=7.1"),
                format!(
                    "{BASE}/Contoso%20Mobile/_apis/wit/attachments?fileName=Spec%20v2.txt&api-version=7.1"
                ),
                format!("{BASE}/_apis/wit/workitems/299?api-version=7.1"),
            ],
            "the work item is read before any byte is uploaded, to its own project"
        );
        assert_eq!(sent[1].body, Some(json!("Spec for 299")));
        assert_eq!(
            sent[2].body,
            Some(json!([{"op": "add", "path": "/relations/-", "value": {
                "rel": "AttachedFile",
                "url": format!("https://dev.azure.com/contoso/_apis/wit/attachments/{SPEC}?fileName=Spec%20v2.txt"),
                "attributes": {"comment": "Spec for the work"}}}]))
        );
    }

    #[test]
    fn create_plans_the_upload_under_dry_run_and_refuses_a_missing_file() {
        let (_dir, path) = spec();
        let plans = dry_run(
            &["ado", "attachment", "create", "299", "--file", &path],
            vec![Answer::json(
                &json!({"id": 299, "fields": {"System.Id": 299}}),
            )],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{BASE}/Fabrikam/_apis/wit/attachments?fileName=Spec%20v2.txt&api-version=7.1")
        );
        assert_eq!(plans[0]["bytes"], 12);

        let (outcome, transport) = ado(
            &[
                "ado",
                "attachment",
                "create",
                "299",
                "--file",
                "/no/such/file.log",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }
}
