//! `acr repo list`.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::acr::{REGISTRIES, Session, catalog, count, metadata, path};
use crate::client::{limited, narrow, parallel, stamp};
use crate::config::Azure;
use crate::graph::inventory;

/// A repository's counts and stamps, which the catalog does not carry. Cached
/// per registry for `[azure] refresh` seconds.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Attributes {
    tag_count: Option<u64>,
    manifest_count: Option<u64>,
    created: Option<String>,
    updated: Option<String>,
}

fn attributes(session: &Session<'_>, repo: &str) -> Result<Attributes> {
    let answer = session.get(&metadata(repo), &path(repo))?;
    Ok(Attributes {
        tag_count: count(&answer["tagCount"]),
        manifest_count: count(&answer["manifestCount"]),
        created: stamp(&answer["createdTime"]),
        updated: stamp(&answer["lastUpdateTime"]),
    })
}

#[derive(clap::Args)]
pub struct RepoListArgs {
    /// Part of the repository name, any case
    name: Option<String>,
    /// Only this registry (repeatable; within [azure] registries)
    #[arg(long)]
    registry: Vec<String>,
    /// Only repositories last pushed to after this
    #[arg(long)]
    since: Option<When>,
    /// Only repositories last pushed to before this: stale ones
    #[arg(long)]
    until: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RepoRow {
    /// `loginserver/repository`: what `acr tag list` takes.
    id: String,
    registry: String,
    repository: String,
    tag_count: Option<u64>,
    manifest_count: Option<u64>,
    /// RFC 3339.
    created: Option<String>,
    /// When something was last pushed to it.
    updated: Option<String>,
}

fn repo_list(ctx: &Ctx, args: RepoListArgs) -> Result<Vec<RepoRow>> {
    let window = args.since.is_some() || args.until.is_some();
    let azure = Azure::load(ctx)?;
    let registries = narrow(
        &inventory(ctx, &azure)?.registries,
        &azure.registries,
        &args.registry,
        REGISTRIES,
        |r| &r.name,
    )?;
    let sessions = registries
        .iter()
        .map(|registry| Session::new(ctx, registry))
        .collect::<Result<Vec<_>>>()?;
    let wanted = args.name.as_deref().map(str::to_ascii_lowercase);
    let mut names: Vec<(usize, String)> = Vec::new();
    let mut failed = Vec::new();
    for (index, listed) in parallel(&sessions, azure.parallel(), catalog)
        .into_iter()
        .enumerate()
    {
        match listed {
            Ok(listed) => names.extend(
                listed
                    .into_iter()
                    .filter(|name| {
                        wanted
                            .as_deref()
                            .is_none_or(|w| name.to_ascii_lowercase().contains(w))
                    })
                    .map(|name| (index, name)),
            ),
            Err(error) => failed.push(error),
        }
    }
    if !failed.is_empty() && failed.len() == sessions.len() {
        return Err(failed.remove(0));
    }
    for error in failed {
        ctx.note(format!("[not read: {error:#}]"));
    }
    let total = names.len();
    // Without a time filter only the rows that will print need attributes.
    if !window {
        names.truncate(args.limit);
    }
    let filled = fill(ctx, &azure, &sessions, &names)?;
    let rows: Vec<RepoRow> = names
        .into_iter()
        .zip(filled)
        .filter(|(_, held)| {
            if !window {
                return true;
            }
            // A repository whose date did not load is in no window.
            held.updated
                .as_deref()
                .and_then(crate::client::parse_stamp)
                .is_some_and(|at| {
                    args.since.is_none_or(|since| at >= since.0)
                        && args.until.is_none_or(|until| at <= until.0)
                })
        })
        .map(|((index, repository), held)| RepoRow {
            id: format!("{}/{repository}", registries[index].login_server),
            registry: registries[index].name.clone(),
            repository,
            tag_count: held.tag_count,
            manifest_count: held.manifest_count,
            created: held.created,
            updated: held.updated,
        })
        .collect();
    if !window && total > rows.len() {
        ctx.note(format!("[{} of {total}; --limit N]", rows.len()));
        return Ok(rows);
    }
    Ok(limited(ctx, rows, args.limit))
}

/// The attributes of each `(registry, repository)`, from the cache where it
/// has them and read side by side where it does not. A repository gone since
/// the catalog, or one this login may list but not read (ABAC), keeps its
/// name with the rest left empty; any other failure is the answer.
fn fill(
    ctx: &Ctx,
    azure: &Azure,
    sessions: &[Session<'_>],
    names: &[(usize, String)],
) -> Result<Vec<Attributes>> {
    let key = |index: usize| format!("acr:attributes:{}", sessions[index].registry.login_server);
    let mut cached: Vec<BTreeMap<String, Attributes>> = (0..sessions.len())
        .map(|index| ctx.cache().get(&key(index)).unwrap_or_default())
        .collect();
    let missing: Vec<&(usize, String)> = names
        .iter()
        .filter(|(index, name)| !cached[*index].contains_key(name))
        .collect();
    let read = parallel(&missing, azure.parallel(), |(index, name)| {
        attributes(&sessions[*index], name)
    });
    let mut skipped = 0;
    let mut touched = vec![false; sessions.len()];
    for ((index, name), result) in missing.into_iter().zip(read) {
        match result {
            Ok(held) => {
                cached[*index].insert(name.clone(), held);
                touched[*index] = true;
            }
            Err(error) if matches!(crate::client::refused_with(&error), Some(403 | 404)) => {
                skipped += 1
            }
            Err(error) => return Err(error),
        }
    }
    for (index, map) in cached.iter().enumerate() {
        if touched[index] {
            ctx.cache().put(&key(index), map, azure.refresh());
        }
    }
    if skipped > 0 {
        ctx.note(format!("[{skipped} repositories' counts and dates did not load (gone, or not readable by this login)]"));
    }
    Ok(names
        .iter()
        .map(|(index, name)| cached[*index].get(name).cloned().unwrap_or_default())
        .collect())
}

command! {
    pub REPO_LIST = ["acr", "repo", "list"], Read,
    "List container image repositories with tag counts and last push",
    keywords: ["image", "images", "repository", "repositories", "pushed", "recent", "docker", "stale", "old", "unused"],
    example: "acr repo list api --since 7d --fields id,tag_count,updated",
    run: repo_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::ACR;
    use crate::testing::{self, azure, exchanged, field, issued, one_registry};

    #[test]
    fn repo_list_fills_counts_and_dates_the_catalog_leaves_out() {
        let (outcome, transport) = azure(
            &[ACR],
            &["acr", "repo", "list"],
            vec![
                one_registry(),
                exchanged(),
                issued("catalog-token"),
                Answer::json(&json!({"repositories": ["team/api", "team/web"]})),
                issued("api-token"),
                Answer::json(
                    &json!({"imageName": "team/api", "tagCount": "48", "manifestCount": 51,
                    "createdTime": "2025-09-11T18:00:00Z", "lastUpdateTime": "2026-09-11T18:00:00Z"}),
                ),
                issued("web-token"),
                Answer::json(&json!({"imageName": "team/web", "tagCount": 3})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["repository"], "team/api");
        assert_eq!(
            rows[0]["tag_count"], 48,
            "a count written as digits is the same count"
        );
        assert_eq!(rows[0]["manifest_count"], 51);
        assert_eq!(rows[0]["updated"], "2026-09-11T18:00:00Z");
        assert_eq!(rows[1]["tag_count"], 3);
        let sent = transport.sent();
        assert_eq!(
            sent.iter()
                .filter(|s| s.url.ends_with("/oauth2/exchange"))
                .count(),
            1,
            "one exchange for every scope"
        );
        assert_eq!(sent[1].url, "https://contosoacr.azurecr.io/oauth2/exchange");
        assert_eq!(field(&sent[1], "grant_type"), Some("access_token"));
        assert_eq!(
            field(&sent[1], "access_token"),
            Some("token@https://containerregistry.azure.net"),
            "the containerregistry audience, not ARM"
        );
        assert!(
            sent[1].authorization.is_none(),
            "a token post carries no bearer"
        );
        assert_eq!(field(&sent[2], "scope"), Some("registry:catalog:*"));
        assert_eq!(field(&sent[2], "refresh_token"), Some("refresh-fixture-1"));
        assert_eq!(
            sent[3].url,
            "https://contosoacr.azurecr.io/acr/v1/_catalog?n=100"
        );
        assert_eq!(
            sent[3].authorization.as_deref(),
            Some("Bearer catalog-token")
        );
        assert_eq!(
            field(&sent[4], "scope"),
            Some("repository:team/api:metadata_read")
        );
        assert_eq!(sent[5].url, "https://contosoacr.azurecr.io/acr/v1/team/api");
    }

    #[test]
    fn attributes_are_cached_per_registry_and_a_gone_repository_keeps_its_name() {
        let temp = tempfile::tempdir().unwrap();
        let run = |argv: &[&str], answers: Vec<Answer>| {
            let transport = agent_cli_core::testing::FakeTransport::answering(answers);
            let mut setup =
                agent_cli_core::Setup::fake(transport.clone()).with_config(testing::SERIAL);
            setup.cache_dir = Some(temp.path().to_owned());
            (agent_cli_core::testing::run(&[ACR], argv, setup), transport)
        };
        let (outcome, _) = run(
            &["acr", "repo", "list"],
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(&json!({"repositories": ["gone", "team/api"]})),
                issued("t1"),
                Answer::status(
                    404,
                    r#"{"errors":[{"code":"NAME_UNKNOWN","message":"repository name not known"}]}"#,
                ),
                issued("t2"),
                Answer::json(&json!({"tagCount": 2, "lastUpdateTime": "2026-09-11T18:00:00Z"})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[0],
            json!({"id": "contosoacr.azurecr.io/gone", "registry": "contosoacr", "repository": "gone"})
        );
        assert!(
            outcome.stderr.contains("1 repositories' counts"),
            "{}",
            outcome.stderr
        );

        let (outcome, transport) = run(
            &["acr", "repo", "list", "api"],
            vec![
                exchanged(),
                issued("t"),
                Answer::json(&json!({"repositories": ["gone", "team/api"]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["tag_count"], 2);
        assert_eq!(
            transport.sent().len(),
            3,
            "the inventory and the attributes came from the cache"
        );

        let api = json!([{"id": "contosoacr.azurecr.io/team/api"}]);
        for (flag, at, want) in [
            ("--since", "2026-09-01", api.clone()),
            ("--since", "2026-09-12T00:00:00Z", json!([])),
            ("--until", "2026-09-12T00:00:00Z", api),
            ("--until", "2026-09-01", json!([])),
        ] {
            let (outcome, _) = run(
                &["acr", "repo", "list", flag, at, "--fields", "id"],
                vec![
                    exchanged(),
                    issued("t"),
                    Answer::json(&json!({"repositories": ["gone", "team/api"]})),
                    issued("t1"),
                    Answer::status(
                        404,
                        r#"{"errors":[{"code":"NAME_UNKNOWN","message":"gone"}]}"#,
                    ),
                ],
            );
            assert_eq!(outcome.code, 0, "{outcome:?}");
            assert_eq!(
                outcome.json(),
                want,
                "last pushed {flag} {at}, and gone has no date"
            );
        }
    }
}
