//! `acr tag list`.

use agent_cli_core::{Ctx, Failure, command, percent_encode};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::acr::{PAGE, Session, image_ref, metadata, path, registry_for};
use crate::client::{stamp, text};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct TagListArgs {
    /// The repository: its exact name (team/api) or id (contosoacr.azurecr.io/team/api)
    repo: String,
    /// The registry that holds it; needed when more than one does
    #[arg(long)]
    registry: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TagRow {
    /// The image, `loginserver/repository:tag`: what `acr manifest get` takes.
    id: String,
    tag: String,
    digest: String,
    /// RFC 3339.
    created: Option<String>,
    updated: Option<String>,
}

fn tag_list(ctx: &Ctx, args: TagListArgs) -> Result<Vec<TagRow>> {
    let azure = Azure::load(ctx)?;
    let image = image_ref(&args.repo);
    if image.reference.is_some() {
        return Err(Failure::usage(format!(
            "{} names one image; tag list takes its repository",
            args.repo
        ))
        .hint(format!("agent-cli acr manifest get {}", args.repo))
        .into());
    }
    let registry = registry_for(ctx, &azure, &image, args.registry.as_deref())?;
    let args = TagListArgs {
        repo: image.repo,
        ..args
    };
    let session = Session::new(ctx, &registry)?;
    let mut tags: Vec<TagRow> = Vec::new();
    let mut last: Option<String> = None;
    // `orderby=timedesc` makes it newest first; paging stops one past the
    // limit, which is enough to say there are more.
    while tags.len() <= args.limit {
        let mut query = format!("{}/_tags?n={PAGE}&orderby=timedesc", path(&args.repo));
        if let Some(last) = &last {
            query.push_str("&last=");
            percent_encode(last, &mut query);
        }
        let page = session.get(&metadata(&args.repo), &query)?;
        let listed: Vec<TagRow> = page["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let tag = text(&entry["name"])?;
                Some(TagRow {
                    id: format!("{}/{}:{tag}", registry.login_server, args.repo),
                    tag,
                    digest: text(&entry["digest"]).unwrap_or_default(),
                    created: stamp(&entry["createdTime"]),
                    updated: stamp(&entry["lastUpdateTime"]),
                })
            })
            .collect();
        let full = listed.len() >= PAGE;
        let previous = last.clone();
        last = listed.last().map(|tag| tag.tag.clone());
        tags.extend(listed);
        if !full || last.is_none() || last == previous {
            break;
        }
    }
    if tags.len() > args.limit {
        ctx.note(format!(
            "[first {} tags, newest first; --limit N for more]",
            args.limit
        ));
        tags.truncate(args.limit);
    }
    Ok(tags)
}

command! {
    pub TAG_LIST = ["acr", "tag", "list"], Read,
    "List an image repository's tags, newest first, with their digests",
    keywords: ["tags", "image", "images", "version", "versions", "latest", "pushed", "docker", "registry"],
    example: "acr tag list team/api --fields id,digest,updated",
    run: tag_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::ACR;
    use crate::testing::{azure, exchanged, issued, one_registry};

    #[test]
    fn tags_keep_the_registrys_newest_first_order_and_stop_at_the_limit() {
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "tag", "list", "team/api", "--limit", "1"],
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(&json!({"tags": [
                    {"name": "1.42.0", "digest": "sha256:ab12", "createdTime": "2026-09-11T18:00:00Z", "lastUpdateTime": "2026-09-11T18:00:00Z"},
                    {"name": "1.41.3", "digest": "sha256:9f01", "createdTime": "2026-09-08T18:00:00Z"},
                ]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "contosoacr.azurecr.io/team/api:1.42.0", "tag": "1.42.0", "digest": "sha256:ab12",
                "created": "2026-09-11T18:00:00Z", "updated": "2026-09-11T18:00:00Z"}])
        );
        assert!(
            outcome.stderr.contains("[first 1 tags"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent()[3].url,
            "https://contosoacr.azurecr.io/acr/v1/team/api/_tags?n=100&orderby=timedesc"
        );
    }
}
