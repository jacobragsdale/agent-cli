use agent_cli_core::{Ctx, Failure, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{API, Ado, Body};
use crate::ids::{is_guid, query_param, web};

use super::guid_in;

#[derive(clap::Args)]
pub struct AttachmentGetArgs {
    /// The attachment: its id, as attachment list prints it, or its URL. Text up to 1 MiB prints; anything else needs --output FILE
    attachment: String,
}

/// One attachment: its text, or where `--output` saved it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AttachmentFile {
    /// The attachment's GUID.
    id: String,
    name: Option<String>,
    /// In bytes.
    size: usize,
    /// The file, when it is text of 1 MiB or less and no --output was given.
    text: Option<String>,
    /// Where --output wrote the bytes.
    saved: Option<String>,
}

/// The most text printed in place of a file.
const INLINE: usize = 1024 * 1024;

fn attachment_get(ctx: &Ctx, args: AttachmentGetArgs) -> Result<AttachmentFile> {
    let ado = Ado::load(ctx)?;
    // Only a URL's `?fileName=` names the file: the answer's
    // Content-Disposition says just `attachment`.
    let (id, name) = parse(&ado, &args.attachment)?;
    let url = ado.api(None, &format!("wit/attachments/{id}"), "", API);
    // The bytes come back whatever Accept asks for. ponytail: the whole
    // file is held in memory (core reads up to 64 MiB, above Azure DevOps's
    // 60 MB); stream it to --output if memory matters.
    let bytes = ado
        .send(ctx, Method::Get, &url, Body::None, None)?
        .into_bytes();
    let size = bytes.len();
    if let Some(saved) = ctx.save(&bytes)? {
        return Ok(AttachmentFile {
            id,
            name,
            size,
            text: None,
            saved: Some(saved.display().to_string()),
        });
    }
    // A NUL byte is how a binary file shows, as in file get. A BOM is no
    // part of the text.
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
                .hint(format!("agent-cli ado attachment get {id} --output FILE"))
                .into(),
        );
    }
    Ok(AttachmentFile {
        id,
        name,
        size,
        text,
        saved: None,
    })
}

/// The GUID, and the file name a URL carries (`?fileName=`).
fn parse(ado: &Ado, raw: &str) -> Result<(String, Option<String>)> {
    let raw = raw.trim();
    if is_guid(raw) {
        return Ok((raw.to_owned(), None));
    }
    let wrong = |why: String| -> anyhow::Error {
        Failure::usage(why)
            .hint("agent-cli ado attachment list ID --fields id,name")
            .into()
    };
    let (_, query) = web(ado, raw)
        .map_err(&wrong)?
        .ok_or_else(|| wrong(format!("{raw:?} is not an attachment id")))?;
    let id = guid_in(raw).ok_or_else(|| wrong(format!("{raw} is not an attachment URL")))?;
    Ok((id, query_param(&query, "fileName")))
}

command! {
    pub ATTACHMENT_GET = ["ado", "attachment", "get"], Read,
    "Show a work item attachment's text, or save the file with --output",
    keywords: ["download", "file", "attached", "screenshot", "read", "save", "log"],
    example: "ado attachment get 098a279a-60b9-40a8-868b-b7fd00c0a439",
    run: attachment_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::super::tests::{LOGO, SPEC};
    use crate::testing::{BASE, ado, urls};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[test]
    fn a_text_attachment_prints_its_text_without_a_bom() {
        let (outcome, transport) = ado(
            &["ado", "attachment", "get", SPEC],
            vec![Answer::bytes(b"\xEF\xBB\xBFSpec for 299")],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": SPEC, "size": 15, "text": "Spec for 299"})
        );
        assert_eq!(
            urls(&transport),
            [format!(
                "{BASE}/_apis/wit/attachments/{SPEC}?api-version=7.1"
            )]
        );
    }

    #[test]
    fn a_binary_attachment_needs_output_which_saves_its_bytes_and_prints_the_row() {
        let (outcome, _) = ado(
            &["ado", "attachment", "get", LOGO],
            vec![Answer::bytes(PNG)],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains(&format!("attachment {LOGO} (16 bytes) is a binary file"))
                && outcome.stderr.contains(&format!(
                    "agent-cli ado attachment get {LOGO} --output FILE"
                )),
            "{}",
            outcome.stderr
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logo.png");
        let url =
            format!("https://dev.azure.com/contoso/_apis/wit/attachments/{LOGO}?fileName=logo.png");
        let (outcome, transport) = ado(
            &[
                "ado",
                "attachment",
                "get",
                &url,
                "--output",
                path.to_str().unwrap(),
            ],
            vec![Answer::bytes(PNG)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": LOGO, "name": "logo.png", "size": 16,
                "saved": path.display().to_string()})
        );
        assert_eq!(std::fs::read(&path).unwrap(), PNG);
        assert_eq!(
            urls(&transport),
            [format!(
                "{BASE}/_apis/wit/attachments/{LOGO}?api-version=7.1"
            )]
        );
    }

    #[test]
    fn text_over_a_mebibyte_needs_output_and_a_foreign_url_is_exit_2() {
        let big = "x".repeat(1024 * 1024 + 1);
        let (outcome, _) = ado(
            &["ado", "attachment", "get", SPEC],
            vec![Answer::bytes(big.as_bytes())],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("is over the 1 MiB printed"),
            "{}",
            outcome.stderr
        );

        for raw in [
            "https://dev.azure.com/other/_apis/wit/attachments/098a279a-60b9-40a8-868b-b7fd00c0a439",
            "Spec.txt",
        ] {
            let (outcome, transport) = ado(&["ado", "attachment", "get", raw], vec![]);
            assert_eq!(outcome.code, 2, "{raw}: {outcome:?}");
            assert!(transport.sent().is_empty());
        }
    }
}
