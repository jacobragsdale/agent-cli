//! `lake path get`: one file or folder's properties and ACL (two HEADs),
//! and for a Parquet file its rows, writer and schema from its footer.

use agent_cli_core::{Ctx, Response, command, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::lake::parquet::{Column, footers};
use crate::lake::{Lake, http_time};

#[derive(clap::Args)]
pub struct PathGetArgs {
    /// The file or folder: an id from lake path list, or its abfss://, wasbs://, https:// or azure:// URL
    path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PathDetail {
    /// `account/container/path`.
    id: String,
    /// file or directory.
    kind: &'static str,
    size: Option<u64>,
    modified: Option<String>,
    etag: Option<String>,
    /// A user's UPN, else an object id; `$superuser` when none was set.
    owner: Option<String>,
    group: Option<String>,
    /// `rwxr-x---`, with `+` when the ACL has more entries.
    permissions: Option<String>,
    /// The POSIX ACL: `user::rwx`, `user:UPN:r-x`, `mask::r-x`, `default:…`.
    acl: Vec<String>,
    /// For a Parquet file, from its footer.
    rows: Option<u64>,
    /// The library that wrote it.
    writer: Option<String>,
    schema: Vec<Column>,
}

fn header(answer: &Response, name: &str) -> Option<String> {
    answer
        .header(name)
        .map(str::trim)
        .filter(|held| !held.is_empty())
        .map(str::to_owned)
}

fn path_get(ctx: &Ctx, args: PathGetArgs) -> Result<PathDetail> {
    let lake = Lake::load(ctx)?;
    let at = lake.resolve(&args.path, true)?;
    let properties = lake.head(&at.head_url())?;
    let access = lake.head(&format!(
        "{}?action=getAccessControl&upn=true",
        at.head_url()
    ))?;
    let directory =
        at.path.is_empty() || properties.header("x-ms-resource-type") == Some("directory");
    let size = header(&properties, "content-length").and_then(|raw| raw.parse().ok());
    let mut detail = PathDetail {
        id: at.id(),
        kind: if directory { "directory" } else { "file" },
        size: size.filter(|_| !directory),
        modified: properties
            .header("last-modified")
            .and_then(http_time)
            .map(utc_time),
        etag: header(&properties, "etag"),
        owner: header(&access, "x-ms-owner"),
        group: header(&access, "x-ms-group"),
        permissions: header(&access, "x-ms-permissions"),
        acl: header(&access, "x-ms-acl")
            .map(|acl| acl.split(',').map(str::to_owned).collect())
            .unwrap_or_default(),
        rows: None,
        writer: None,
        schema: Vec::new(),
    };
    if let (false, true, Some(size)) = (directory, at.path.ends_with(".parquet"), size) {
        match footers(&lake, &[(at.clone(), size)]).remove(0) {
            Ok(footer) => {
                detail.rows = Some(footer.rows);
                detail.writer = footer.writer;
                detail.schema = footer.schema;
            }
            Err(error) => ctx.note(format!("[its footer was not read: {error:#}]")),
        }
    }
    Ok(detail)
}

command! {
    pub PATH_GET = ["lake", "path", "get"], Read,
    "Show a Data Lake file's size, time, owner, ACL, and Parquet schema and rows",
    keywords: ["file", "properties", "metadata", "permissions", "acl", "posix", "columns", "footer", "count", "writer"],
    example: "lake path get contosolake/raw/sales/orders/date=2026-09-29/part-0001.parquet --fields size,modified,rows,schema",
    run: path_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Method;
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::LAKE;
    use crate::lake::testing::{tail, world};
    use crate::testing::{self, azure};

    #[test]
    fn a_parquet_file_shows_its_properties_its_acl_and_its_footer_without_a_download() {
        let bytes = world("contosolake/raw/sales/orders/date=2026-09-28/part-0001.parquet");
        let size = bytes.len().to_string();
        let (outcome, transport) = azure(
            &[LAKE],
            &[
                "lake",
                "path",
                "get",
                "https://contosolake.blob.core.windows.net/raw/sales/orders/date=2026-09-29/part-0001.parquet?sig=x",
            ],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                Answer::ok("")
                    .with_header("Content-Length", &size)
                    .with_header("Last-Modified", "Mon, 29 Sep 2026 06:02:11 GMT")
                    .with_header("ETag", "\"0x8DDFF0000000002\"")
                    .with_header("x-ms-resource-type", "file"),
                Answer::ok("")
                    .with_header("x-ms-owner", "etl@contoso.example")
                    .with_header("x-ms-group", "$superuser")
                    .with_header("x-ms-permissions", "rw-r-----+")
                    .with_header(
                        "x-ms-acl",
                        "user::rw-,user:etl@contoso.example:rw-,group::r--,mask::rw-,other::---",
                    ),
                tail(&bytes),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["id"],
            "contosolake/raw/sales/orders/date=2026-09-29/part-0001.parquet"
        );
        assert_eq!(
            (got["kind"].clone(), got["rows"].clone()),
            (json!("file"), json!(4))
        );
        assert_eq!(got["modified"], "2026-09-29T06:02:11Z");
        assert_eq!(got["acl"][1], "user:etl@contoso.example:rw-");
        assert_eq!(
            got["schema"],
            json!([{"name": "order_id", "type": "INT64"}, {"name": "customer_id", "type": "INT64"},
                {"name": "total", "type": "DOUBLE"}, {"name": "placed_at", "type": "TIMESTAMP(MICROS, UTC)"}])
        );
        let sent = transport.sent();
        let url = "https://contosolake.dfs.core.windows.net/raw/sales/orders/date%3D2026-09-29/part-0001.parquet";
        assert_eq!((sent[1].method, sent[1].url.as_str()), (Method::Head, url));
        assert_eq!(
            sent[2].url,
            format!("{url}?action=getAccessControl&upn=true")
        );
        assert_eq!(sent[3].url, url);
        let range = format!("bytes=0-{}", bytes.len() - 1);
        assert!(
            sent[3].headers.contains(&("Range".into(), range)),
            "{:?}",
            sent[3].headers
        );
    }

    #[test]
    fn a_refused_head_names_the_data_role_and_access_get() {
        let (outcome, _) = azure(
            &[LAKE],
            &["lake", "path", "get", "contosolake/curated/finance/ledger"],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                Answer::status(403, "")
                    .with_header("x-ms-error-code", "AuthorizationPermissionMismatch"),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("control plane only"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains(
                "agent-cli lake access get contosolake/curated/finance/ledger --principal @me"
            ),
            "{}",
            outcome.stderr
        );
    }
}
