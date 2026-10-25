use agent_cli_core::{Ctx, Exit, Failure, Method, command, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Call, Confluence, text};
use crate::ids::{AttachmentRef, attachment, quote};

use super::EXPAND;

#[derive(clap::Args)]
pub struct AttachmentGetArgs {
    /// The attachment: its id, as attachment list prints it, or its URL. Text up to 1 MiB prints; anything else needs --output FILE
    attachment: String,
}

/// One attachment: its text, or where `--output` saved it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AttachmentFile {
    id: String,
    page: Option<String>,
    name: Option<String>,
    media_type: Option<String>,
    /// In bytes.
    size: usize,
    /// The file, when it is text of 1 MiB or less and no --output was given.
    text: Option<String>,
    /// Where --output wrote the bytes.
    saved: Option<String>,
}

/// The most text printed in place of a file.
const INLINE: usize = 1024 * 1024;

/// The file comes from its `_links.download`, under the site with the
/// token like any call: Data Center serves it itself.
fn attachment_get(ctx: &Ctx, args: AttachmentGetArgs) -> Result<AttachmentFile> {
    let confluence = Confluence::load(ctx)?;
    let found = match attachment(&confluence.site, &args.attachment)? {
        AttachmentRef::Id(id) => {
            let url = confluence.api(&format!("/content/{id}"), &[("expand", EXPAND.to_owned())]);
            match confluence.get(ctx, &url) {
                Ok(found) if found["type"] == "attachment" => found,
                Ok(_) => return Err(missing(&id)),
                Err(error) if status_of(&error) == Some(404) => return Err(missing(&id)),
                Err(error) => return Err(error),
            }
        }
        AttachmentRef::Named { page, name } => {
            let url = confluence.api(
                &format!("/content/{page}/child/attachment"),
                &[("filename", name.clone()), ("expand", EXPAND.to_owned())],
            );
            let found = confluence.get(ctx, &url)?;
            found["results"]
                .as_array()
                .and_then(|files| files.first())
                .cloned()
                .ok_or_else(|| {
                    Failure::not_found(format!("page {page} has no file named {name:?}"))
                        .hint(format!("agent-cli confluence attachment list {page}"))
                })?
        }
    };
    let id = text(&found["id"]).unwrap_or_default();
    let name = text(&found["title"]);
    let Some(download) = text(&found["_links"]["download"]) else {
        return Err(Failure::new(
            Exit::Failed,
            format!("attachment {id} answered no download link"),
        )
        .into());
    };
    let bytes = confluence
        .read(ctx, Call::new(Method::Get, &confluence.web(&download)))?
        .into_bytes();
    let size = bytes.len();
    let mut file = AttachmentFile {
        id: id.clone(),
        page: text(&found["container"]["id"]),
        name,
        media_type: text(&found["extensions"]["mediaType"]),
        size,
        text: None,
        saved: None,
    };
    if let Some(saved) = ctx.save(&bytes)? {
        file.saved = Some(saved.display().to_string());
        return Ok(file);
    }
    // A NUL byte is how a binary file shows; a BOM is no part of the text.
    let text = String::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'))
        .map(|text| text.trim_start_matches('\u{feff}').to_owned());
    let why = match &text {
        Some(_) if size > INLINE => "is over the 1 MiB printed",
        Some(_) => "",
        None => "is a binary file",
    };
    if !why.is_empty() {
        return Err(
            Failure::usage(format!("attachment {id} ({size} bytes) {why}"))
                .hint(format!(
                    "agent-cli confluence attachment get {} --output FILE",
                    quote(&id)
                ))
                .into(),
        );
    }
    file.text = text;
    Ok(file)
}

fn missing(id: &str) -> anyhow::Error {
    Failure::not_found(format!("no attachment {id}, or you may not see it"))
        .hint("agent-cli confluence attachment list PAGE --fields id,name")
        .into()
}

command! {
    pub ATTACHMENT_GET = ["confluence", "attachment", "get"], Read,
    "Show a page attachment's text, or save the file with --output",
    keywords: ["download", "file", "attached", "save", "fetch", "log", "csv", "image"],
    example: "confluence attachment get att7001",
    run: attachment_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{API, BASE, TOKEN, confluence, urls};

    fn meta() -> Answer {
        Answer::json(
            &json!({"id": "7001", "type": "attachment", "title": "changes-v1.4.2.txt",
            "container": {"id": "1201", "type": "page"}, "version": {"number": 1},
            "extensions": {"mediaType": "text/plain", "fileSize": 24, "comment": ""},
            "_links": {"download": "/download/attachments/1201/changes-v1.4.2.txt?version=1&modificationDate=1790000000000&api=v2"}}),
        )
    }

    #[test]
    fn a_text_file_downloads_from_its_link_with_the_token() {
        let (outcome, transport) = confluence(
            &["confluence", "attachment", "get", "att7001"],
            vec![meta(), Answer::bytes(b"\xEF\xBB\xBFPR 431: retry on 429\n")],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "7001", "page": "1201", "name": "changes-v1.4.2.txt", "media_type": "text/plain",
                "size": 24, "text": "PR 431: retry on 429\n"})
        );
        assert_eq!(
            urls(&transport),
            [
                format!("{API}/content/7001?expand=version%2Ccontainer"),
                format!(
                    "{BASE}/download/attachments/1201/changes-v1.4.2.txt?version=1&modificationDate=1790000000000&api=v2"
                ),
            ]
        );
        assert_eq!(
            transport.sent()[1].authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
    }

    #[test]
    fn a_file_url_is_found_by_its_name() {
        let found = json!({"results": [{"id": "7001", "type": "attachment", "title": "rollout plan.png",
            "container": {"id": "1201"}, "extensions": {"mediaType": "image/png"},
            "_links": {"download": "/download/attachments/1201/rollout%20plan.png?version=1&api=v2"}}]});
        let (outcome, transport) = confluence(
            &[
                "confluence",
                "attachment",
                "get",
                &format!("{BASE}/download/attachments/1201/rollout%20plan.png?api=v2"),
            ],
            vec![
                Answer::json(&found),
                Answer::bytes(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"),
            ],
        );
        assert_eq!(outcome.code, 2, "a binary file needs --output: {outcome:?}");
        assert_eq!(
            urls(&transport)[0],
            format!(
                "{API}/content/1201/child/attachment?filename=rollout+plan.png&expand=version%2Ccontainer"
            )
        );
    }

    #[test]
    fn a_binary_file_needs_output() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        let answers = || vec![meta(), Answer::bytes(png)];
        let (outcome, _) = confluence(&["confluence", "attachment", "get", "7001"], answers());
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli confluence attachment get 7001 --output FILE")
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.png");
        let (outcome, _) = confluence(
            &[
                "confluence",
                "attachment",
                "get",
                "7001",
                "--output",
                path.to_str().unwrap(),
            ],
            answers(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(std::fs::read(&path).unwrap(), png);
    }
}
