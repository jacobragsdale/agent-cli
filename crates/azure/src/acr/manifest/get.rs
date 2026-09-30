//! `acr manifest get`.

use agent_cli_core::{Ctx, Failure, command, percent_encode};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::acr::{Session, count, image_ref, metadata, path, registry_for};
use crate::client::{stamp, text};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct ManifestGetArgs {
    /// The image: contosoacr.azurecr.io/team/api:1.42.0, team/api@sha256:…, or a repository
    image: String,
    /// A tag (1.42.0) or a digest (sha256:…), when IMAGE names only the repository
    reference: Option<String>,
    /// The registry that holds it; needed when more than one does
    #[arg(long)]
    registry: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ManifestRow {
    digest: String,
    /// Bytes, as the registry counts them.
    size: Option<u64>,
    /// None for a multi-arch index.
    architecture: Option<String>,
    os: Option<String>,
    /// RFC 3339.
    created: Option<String>,
    /// Every tag pointing at it.
    tags: Vec<String>,
    /// What `docker pull` takes, pinned to the digest.
    pull: String,
}

fn manifest_get(ctx: &Ctx, args: ManifestGetArgs) -> Result<ManifestRow> {
    let azure = Azure::load(ctx)?;
    let image = image_ref(&args.image);
    let reference = match (image.reference.clone(), args.reference) {
        (Some(held), Some(given)) if held != given => {
            return Err(Failure::usage(format!(
                "{} names {held}, and REFERENCE says {given}",
                args.image
            ))
            .into());
        }
        (Some(held), _) => held,
        (None, Some(given)) => given,
        (None, None) => {
            return Err(
                Failure::usage(format!("{} names no tag or digest", args.image))
                    .hint(format!(
                        "agent-cli acr tag list {} --fields id,updated",
                        args.image
                    ))
                    .into(),
            );
        }
    };
    let registry = registry_for(ctx, &azure, &image, args.registry.as_deref())?;
    let repo = image.repo;
    let session = Session::new(ctx, &registry)?;
    let mut encoded = String::new();
    percent_encode(&reference, &mut encoded);
    let reference = encoded;
    // A digest's colon survives as itself; the registry reads either form.
    let reference = reference.replace("%3A", ":");
    let answer = session.get(
        &metadata(&repo),
        &format!("{}/_manifests/{reference}", path(&repo)),
    )?;
    // The singular call nests everything under `manifest`.
    let held = if answer["manifest"].is_object() {
        &answer["manifest"]
    } else {
        &answer
    };
    let digest = text(&held["digest"]).unwrap_or(reference);
    Ok(ManifestRow {
        pull: format!("{}/{}@{digest}", registry.login_server, repo),
        size: count(&held["imageSize"]),
        architecture: text(&held["architecture"]),
        os: text(&held["os"]),
        created: stamp(&held["createdTime"]),
        tags: held["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(text)
            .collect(),
        digest,
    })
}

command! {
    pub MANIFEST_GET = ["acr", "manifest", "get"], Read,
    "Describe one image by tag or digest: size, architecture, os, tags on it",
    keywords: ["image", "digest", "sha256", "pull", "reference", "arch", "platform", "size"],
    example: "acr manifest get contosoacr.azurecr.io/team/api:1.42.0 --fields digest,created,tags,pull",
    run: manifest_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::ACR;
    use crate::testing::{azure, exchanged, issued, one_registry};

    #[test]
    fn a_manifest_by_tag_reads_from_under_its_own_key_with_a_pinned_pull_reference() {
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "manifest", "get", "team/api", "1.42.0"],
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(
                    &json!({"registry": "contosoacr.azurecr.io", "imageName": "team/api", "manifest": {
                        "digest": "sha256:ab12ef0199", "imageSize": 84_200_000_u64, "createdTime": "2026-09-11T18:00:00Z",
                        "architecture": "amd64", "os": "linux", "tags": ["1.42.0", "1.42"],
                    }}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let manifest = outcome.json();
        assert_eq!(manifest["size"], 84_200_000);
        assert_eq!(manifest["tags"], json!(["1.42.0", "1.42"]));
        assert_eq!(
            manifest["pull"],
            "contosoacr.azurecr.io/team/api@sha256:ab12ef0199"
        );
        assert_eq!(
            transport.sent()[3].url,
            "https://contosoacr.azurecr.io/acr/v1/team/api/_manifests/1.42.0"
        );

        let (_, transport) = azure(
            &[ACR],
            &["acr", "manifest", "get", "team/api", "sha256:ff"],
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(&json!({"manifest": {"digest": "sha256:ff"}})),
            ],
        );
        assert_eq!(
            transport.sent()[3].url,
            "https://contosoacr.azurecr.io/acr/v1/team/api/_manifests/sha256:ff"
        );
    }
}
