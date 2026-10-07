use std::path::Path;

use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Call, Confluence, text};
use crate::ids::current_page;

#[derive(clap::Args)]
pub struct AttachmentCreateArgs {
    /// The page: its id, KEY:Title or its URL
    page: String,
    /// The file to attach; one of the same name on the page gets a new version
    #[arg(long)]
    file: String,
    /// A note shown beside the attachment
    #[arg(long)]
    comment: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Attached {
    /// What attachment get takes.
    id: String,
    page: String,
    name: String,
    /// In bytes.
    size: usize,
    version: Option<u64>,
}

/// ponytail: the file is held in memory to send; Confluence's own upload
/// limit (site-configured) stops anything that would hurt.
const MAX_BYTES: u64 = 100 * 1024 * 1024;

/// v1's `PUT …/child/attachment` (v2 has no upload) creates the file or
/// adds a version of one with its name, as multipart built here, with the
/// `X-Atlassian-Token: no-check` header its XSRF check wants.
fn attachment_create(ctx: &Ctx, args: AttachmentCreateArgs) -> Result<Attached> {
    let confluence = Confluence::load(ctx)?;
    let path = Path::new(&args.file);
    let unreadable =
        |error: std::io::Error| Failure::usage(format!("cannot read {}: {error}", args.file));
    let size = std::fs::metadata(path).map_err(unreadable)?.len();
    if size > MAX_BYTES {
        return Err(Failure::usage(format!(
            "{} is {size} bytes, over the 100 MiB this sends",
            args.file
        ))
        .into());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Failure::usage(format!("{} names no file", args.file)))?
        .to_owned();
    let bytes = std::fs::read(path).map_err(unreadable)?;
    let id = current_page(ctx, &confluence, &args.page)?;
    // The page is read first so a wrong id is exit 4, not an upload's 404.
    confluence.content(ctx, &id, &[])?;
    let (boundary, body) = multipart(&name, &bytes, args.comment.as_deref());
    let url = confluence.v1(&format!("/content/{id}/child/attachment"), &[]);
    let call = Call::new(Method::Put, &url)
        .header(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .header("X-Atlassian-Token", "no-check")
        .bytes(body);
    let answer = confluence.write(ctx, Effect::Write, call)?.json()?;
    let file = answer["results"]
        .as_array()
        .and_then(|files| files.first())
        .unwrap_or(&answer);
    Ok(Attached {
        id: text(&file["id"]).unwrap_or_default(),
        page: id,
        name: text(&file["title"]).unwrap_or(name),
        size: bytes.len(),
        version: file["version"]["number"].as_u64(),
    })
}

/// The form: the file, `minorEdit` (no notification) and the comment.
fn multipart(name: &str, bytes: &[u8], comment: Option<&str>) -> (String, Vec<u8>) {
    let mut boundary = "agent-cli-boundary-0".to_owned();
    let mut n = 0;
    while bytes
        .windows(boundary.len())
        .any(|window| window == boundary.as_bytes())
    {
        n += 1;
        boundary = format!("agent-cli-boundary-{n}");
    }
    let name = name.replace(['"', '\r', '\n'], "_");
    let mut body = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(
        format!(
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"minorEdit\"\r\n\r\ntrue\r\n"
        )
        .as_bytes(),
    );
    if let Some(comment) = comment {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"comment\"\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{comment}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

command! {
    pub ATTACHMENT_CREATE = ["confluence", "attachment", "create"], Write,
    "Attach a file to a page, or add a new version of one with its name",
    keywords: ["upload", "attach", "file", "add", "image", "log", "csv"],
    example: "confluence attachment create 1201 --file changes-v1.4.3.txt --comment 'Changelog'",
    run: attachment_create,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::multipart;
    use crate::testing::{V1, confluence, dry_run, page};

    #[test]
    fn a_file_goes_up_as_multipart_with_the_xsrf_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changes.txt");
        std::fs::write(&path, "retry on 429").unwrap();
        let file = path.to_str().unwrap();
        let plans = dry_run(
            &[
                "confluence",
                "attachment",
                "create",
                "1201",
                "--file",
                file,
                "--comment",
                "Changelog",
            ],
            vec![Answer::json(&page("1201", "Release notes", 3, ""))],
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(
            plans[0]["url"],
            format!("{V1}/content/1201/child/attachment")
        );
        // Masked as every header named like a token is; it is sent as no-check.
        assert_eq!(plans[0]["headers"]["X-Atlassian-Token"], "***");
        assert_eq!(
            plans[0]["headers"]["Content-Type"],
            "multipart/form-data; boundary=agent-cli-boundary-0"
        );

        let answer = json!({"results": [{"id": "att7003", "title": "changes.txt", "version": {"number": 1}}]});
        let (outcome, transport) = confluence(
            &["confluence", "attachment", "create", "1201", "--file", file],
            vec![
                Answer::json(&page("1201", "Release notes", 3, "")),
                Answer::json(&answer),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "att7003", "page": "1201", "name": "changes.txt", "size": 12, "version": 1})
        );
        let sent = transport.sent();
        let body = sent[1].body.as_ref().unwrap().as_str().unwrap().to_owned();
        assert!(body.contains("filename=\"changes.txt\"\r\nContent-Type: application/octet-stream\r\n\r\nretry on 429\r\n"), "{body}");
        assert!(body.contains("name=\"minorEdit\"\r\n\r\ntrue"), "{body}");
    }

    #[test]
    fn the_boundary_never_occurs_in_the_file() {
        let (boundary, _) = multipart("x", b"--agent-cli-boundary-0 inside", None);
        assert_eq!(boundary, "agent-cli-boundary-1");
    }
}
