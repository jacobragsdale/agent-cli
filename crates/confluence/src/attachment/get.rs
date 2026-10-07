use agent_cli_core::{Ctx, Exit, Failure, Method, Request, Secret, command, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Call, Confluence, text};
use crate::ids::{AttachmentRef, attachment, quote};

#[derive(clap::Args)]
pub struct AttachmentGetArgs {
    /// The attachment: its id (att7001), as attachment list prints it, or its URL. Text up to 1 MiB prints; anything else needs --output FILE
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

/// Two hops: v1's download answers (on Cloud) with a redirect to a
/// pre-signed URL on Atlassian's media host, fetched without the
/// credential. That URL is a bearer secret for about a day, so it is never
/// printed: a failure names the attachment instead. The legacy
/// `/download/attachments/…` link has refused API tokens since 2026-04.
fn attachment_get(ctx: &Ctx, args: AttachmentGetArgs) -> Result<AttachmentFile> {
    let confluence = Confluence::load(ctx)?;
    let id = match attachment(&confluence.site, &args.attachment)? {
        AttachmentRef::Id(id) => id,
        AttachmentRef::Named { page, name } => {
            let url = confluence.v2(
                &format!("/pages/{page}/attachments"),
                &[("filename", name.clone())],
            );
            let found = confluence.get(ctx, &url)?;
            found["results"]
                .as_array()
                .and_then(|files| files.first())
                .and_then(|file| text(&file["id"]))
                .ok_or_else(|| {
                    Failure::not_found(format!("page {page} has no file named {name:?}"))
                        .hint(format!("agent-cli confluence attachment list {page}"))
                })?
        }
    };
    let found = confluence.get(ctx, &confluence.v2(&format!("/attachments/{id}"), &[]))?;
    let page = text(&found["pageId"]).or_else(|| text(&found["blogPostId"]));
    let name = text(&found["title"]);
    let Some(container) = &page else {
        return Err(
            Failure::usage(format!("attachment {id} is on no page or blog post"))
                .hint(format!("agent-cli confluence attachment get {id}"))
                .into(),
        );
    };
    let url = confluence.v1(
        &format!("/content/{container}/child/attachment/{id}/download"),
        &[],
    );
    let answer = confluence.read(ctx, Call::new(Method::Get, &url).keep_redirect())?;
    let bytes = if (300..400).contains(&answer.status) {
        let location = answer.header("Location").unwrap_or_default().to_owned();
        let location = Secret::new(location);
        let what = format!("attachment {id} ({})", name.as_deref().unwrap_or("?"));
        if !location.expose().starts_with("https://") {
            return Err(Failure::new(
                Exit::Failed,
                format!("{what} redirected somewhere other than https"),
            )
            .into());
        }
        ctx.read(Request::get(location.expose()))
            .map_err(|error| media_failure(&error, &what))?
            .into_bytes()
    } else {
        answer.into_bytes()
    };
    let size = bytes.len();
    let mut file = AttachmentFile {
        id: id.clone(),
        page,
        name,
        media_type: text(&found["mediaType"]),
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

/// A refusal from the media host, without the URL that carried it.
fn media_failure(error: &anyhow::Error, what: &str) -> anyhow::Error {
    let (exit, said) = match status_of(error) {
        Some(404) => (Exit::NotFound, "answered 404".to_owned()),
        Some(status) => (Exit::Failed, format!("answered {status}")),
        None => (Exit::Failed, "could not be reached".to_owned()),
    };
    let mut failure = Failure::new(exit, format!("downloading {what}: the media host {said}"));
    failure.status = status_of(error);
    failure
        .hint("the download link lasts about a day: run the command again")
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

    use crate::testing::{TOKEN, V1, V2, confluence, urls};

    const MEDIA: &str = "https://api.media.atlassian.com/file/f-7001/binary?token=eyJhbGciOiJIUzI1NiJ9.e30.c2lnbmF0dXJl&client=c1&name=changes.txt";

    fn meta() -> Answer {
        Answer::json(
            &json!({"id": "att7001", "title": "changes-v1.4.2.txt", "pageId": "1201",
            "mediaType": "text/plain", "fileSize": 23}),
        )
    }

    #[test]
    fn a_text_file_comes_through_the_redirect_without_the_credential() {
        let (outcome, transport) = confluence(
            &["confluence", "attachment", "get", "7001"],
            vec![
                meta(),
                Answer::status(302, "").with_header("Location", MEDIA),
                Answer::bytes(b"\xEF\xBB\xBFPR 431: retry on 429\n"),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "att7001", "page": "1201", "name": "changes-v1.4.2.txt", "media_type": "text/plain",
                "size": 24, "text": "PR 431: retry on 429\n"})
        );
        let sent = transport.sent();
        assert_eq!(
            urls(&transport),
            [
                format!("{V2}/attachments/att7001"),
                format!("{V1}/content/1201/child/attachment/att7001/download"),
                MEDIA.to_owned(),
            ]
        );
        assert!(sent[1].authorization.is_some());
        assert!(
            sent[2].authorization.is_none(),
            "the media host never sees the token"
        );
        assert!(!format!("{:?}", sent[2]).contains(TOKEN));
    }

    #[test]
    fn a_media_refusal_names_the_attachment_never_the_link() {
        let (outcome, _) = confluence(
            &["confluence", "attachment", "get", "att7001"],
            vec![
                meta(),
                Answer::status(302, "").with_header("Location", MEDIA),
                Answer::status(403, "expired"),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(outcome.stderr.contains(
            "downloading attachment att7001 (changes-v1.4.2.txt): the media host answered 403"
        ));
        assert!(
            !outcome.stderr.contains("api.media") && !outcome.stderr.contains("eyJ"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_binary_file_needs_output() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        let answers = || {
            vec![
                meta(),
                Answer::status(302, "").with_header("Location", MEDIA),
                Answer::bytes(png),
            ]
        };
        let (outcome, _) = confluence(&["confluence", "attachment", "get", "att7001"], answers());
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli confluence attachment get att7001 --output FILE")
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.png");
        let (outcome, _) = confluence(
            &[
                "confluence",
                "attachment",
                "get",
                "att7001",
                "--output",
                path.to_str().unwrap(),
            ],
            answers(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(std::fs::read(&path).unwrap(), png);
    }
}
