//! `acr tag delete`: untags an image (`DELETE /acr/v1/{repo}/_tags/{tag}`
//! with a `delete` scope). The manifest stays, and with it every other tag
//! on it; the answer names its digest, so the tag can be put back.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::acr::{Session, image_ref, metadata, path, registry_for};
use crate::client::text;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct TagDeleteArgs {
    /// The tag: loginserver/repo:tag as acr tag list prints it, or repo:tag
    image: String,
    /// The registry that holds it; needed when more than one does
    #[arg(long)]
    registry: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Deleted {
    /// The tag that is gone, `loginserver/repository:tag`.
    id: String,
    /// The manifest it pointed at, still in the registry.
    digest: Option<String>,
}

fn tag_delete(ctx: &Ctx, args: TagDeleteArgs) -> Result<Deleted> {
    let azure = Azure::load(ctx)?;
    let image = image_ref(&args.image);
    let Some(tag) = image
        .reference
        .clone()
        .filter(|reference| !reference.starts_with("sha256:"))
    else {
        return Err(Failure::usage(format!(
            "{} names no tag; tag delete takes repo:tag (deleting a manifest by digest is not covered)",
            args.image
        ))
        .hint(format!(
            "agent-cli acr tag list {} --fields id,digest",
            image.repo
        ))
        .into());
    };
    let registry = registry_for(ctx, &azure, &image, args.registry.as_deref())?;
    let session = Session::new(ctx, &registry)?;
    let at = format!("{}/_tags/{tag}", path(&image.repo));
    let held = session.get(&metadata(&image.repo), &at).map_err(|error| {
        if crate::client::refused_with(&error) == Some(404) {
            return Failure::not_found(format!("no tag {tag} in {}", image.repo))
                .hint(format!(
                    "agent-cli acr tag list {}/{} --fields id",
                    registry.login_server, image.repo
                ))
                .into();
        }
        error
    })?;
    session.delete(&format!("repository:{}:delete", image.repo), &at)?;
    Ok(Deleted {
        id: format!("{}/{}:{tag}", registry.login_server, image.repo),
        digest: text(&held["tag"]["digest"]),
    })
}

command! {
    pub TAG_DELETE = ["acr", "tag", "delete"], Destructive,
    "Delete an image tag from a registry (the manifest and its other tags stay)",
    keywords: ["untag", "remove", "image", "docker", "cleanup"],
    example: "acr tag delete contosoacr.azurecr.io/api:v1.4.0 --yes",
    run: tag_delete,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Method;
    use agent_cli_core::testing::{Answer, assert_dry_run};
    use serde_json::json;

    use crate::ACR;
    use crate::testing::{azure, exchanged, field, issued, one_registry};

    fn tag() -> Answer {
        Answer::json(
            &json!({"registry": "contosoacr.azurecr.io", "imageName": "api",
            "tag": {"name": "v1.4.0", "digest": "sha256:d8df"}}),
        )
    }

    #[test]
    fn a_tag_is_read_then_deleted_with_a_delete_scope() {
        let plans = assert_dry_run(
            &[ACR],
            &["acr", "tag", "delete", "contosoacr.azurecr.io/api:v1.4.0"],
            vec![one_registry(), exchanged(), issued("tok-read-1"), tag()],
        );
        assert_eq!(plans[0]["method"], "DELETE");
        assert_eq!(
            plans[0]["url"],
            "https://contosoacr.azurecr.io/acr/v1/api/_tags/v1.4.0"
        );

        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "tag", "delete", "api:v1.4.0", "--yes"],
            vec![
                one_registry(),
                exchanged(),
                issued("tok-read-1"),
                tag(),
                issued("tok-delete-1"),
                Answer::status(202, ""),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "contosoacr.azurecr.io/api:v1.4.0", "digest": "sha256:d8df"})
        );
        let sent = transport.sent();
        assert_eq!(field(&sent[4], "scope"), Some("repository:api:delete"));
        assert_eq!(sent[5].method, Method::Delete);
        assert_eq!(
            sent[5].authorization.as_deref(),
            Some("Bearer tok-delete-1")
        );

        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "tag", "delete", "api:v1.4.0"],
            vec![one_registry(), exchanged(), issued("tok-read-1"), tag()],
        );
        assert_eq!(outcome.code, 2, "a delete needs --yes: {outcome:?}");
        assert!(
            transport
                .sent()
                .iter()
                .all(|sent| sent.method != Method::Delete)
        );
    }

    #[test]
    fn a_digest_or_a_missing_tag_is_refused() {
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "tag", "delete", "api@sha256:d8df", "--yes"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
        let (outcome, _) = azure(
            &[ACR],
            &["acr", "tag", "delete", "api:v9", "--yes"],
            vec![
                one_registry(),
                exchanged(),
                issued("tok-read-1"),
                Answer::status(
                    404,
                    r#"{"errors":[{"code":"TAG_UNKNOWN","message":"the specified tag does not exist"}]}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
