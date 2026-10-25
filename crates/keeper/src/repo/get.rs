//! `keeper repo get`: `GET /repositories/{key}`, with a virtual
//! repository's members and a remote one's cache lifetime.

use agent_cli_core::{Ctx, command, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use super::RepoRow;
use crate::client::{Keeper, list, segment, stamp, text};
use crate::ids;

#[derive(clap::Args)]
pub struct RepoGetArgs {
    /// The repository: its key, as repo list prints it, or its web, API or pip index URL
    repo: String,
}

/// One repository and how it serves.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RepoDetail {
    #[serde(flatten)]
    repo: RepoRow,
    description: Option<String>,
    /// The most bytes it may hold, when capped.
    quota: Option<u64>,
    /// A virtual repository's members, in the order a request searches them.
    members: Vec<Member>,
    /// How long a remote repository keeps an upstream answer before asking
    /// again, in seconds.
    cache_ttl: Option<u64>,
    /// Keeps every upload of a path as a revision (others refuse a path
    /// twice).
    versioning: Option<bool>,
    /// Curation rules decide what may be downloaded through it.
    curation: Option<bool>,
    updated: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Member {
    /// The member's repository key.
    id: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    priority: Option<i64>,
}

fn repo_get(ctx: &Ctx, args: RepoGetArgs) -> Result<RepoDetail> {
    let keeper = Keeper::load(ctx)?;
    let key = ids::repo(&keeper, &args.repo)?;
    let found = keeper.repo(ctx, &key)?;
    let kind = text(&found["repo_type"]).unwrap_or_default();
    let mut members = Vec::new();
    if kind == "virtual" {
        let url = keeper.api(&format!("/repositories/{}/members", segment(&key)), &[]);
        match keeper.get(ctx, &url) {
            Ok(answer) => {
                let mut listed = list(&answer["members"]);
                listed.sort_by_key(|member| member["priority"].as_i64().unwrap_or(i64::MAX));
                members = listed
                    .iter()
                    .filter_map(|member| {
                        Some(Member {
                            id: text(&member["member_repo_key"])?,
                            kind: text(&member["member_repo_type"]),
                            priority: member["priority"].as_i64(),
                        })
                    })
                    .collect();
            }
            // Keeper shows members only to a signed-in caller.
            Err(error) if status_of(&error) == Some(401) => {
                ctx.note("[its members need a token under [keeper]]");
            }
            Err(error) => return Err(error),
        }
    }
    let mut cache_ttl = None;
    if kind == "remote" {
        let url = keeper.api(&format!("/repositories/{}/cache-ttl", segment(&key)), &[]);
        match keeper.get(ctx, &url) {
            Ok(answer) => cache_ttl = answer["cache_ttl_seconds"].as_u64(),
            Err(error) if status_of(&error).is_some() => {}
            Err(error) => return Err(error),
        }
    }
    Ok(RepoDetail {
        repo: RepoRow::of(&found),
        description: text(&found["description"]),
        quota: found["quota_bytes"].as_u64(),
        members,
        cache_ttl,
        versioning: found["versioning_enabled"].as_bool(),
        curation: found["curation_enabled"].as_bool(),
        updated: stamp(&found["updated_at"]),
    })
}

command! {
    pub REPO_GET = ["keeper", "repo", "get"], Read,
    "Show an Artifact Keeper repository: format, upstream, members, cache, size",
    keywords: ["feed", "index", "proxy", "upstream", "virtual", "members", "pip", "pypi", "visibility", "quota"],
    example: "keeper repo get pypi-proxy",
    run: repo_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{API, BASE, anonymous, keeper_run, repository, urls};

    #[test]
    fn a_pip_index_url_names_the_virtual_repository_and_its_members_print_in_order() {
        let (outcome, transport) = keeper_run(
            &[
                "keeper",
                "repo",
                "get",
                &format!("{BASE}/pypi/pypi-all/simple/"),
            ],
            vec![
                Answer::json(&repository("pypi-all", "pypi", "virtual")),
                Answer::json(&json!({"members": [
                    {"id": "m2", "member_repo_key": "pypi-proxy", "member_repo_type": "remote", "priority": 2},
                    {"id": "m1", "member_repo_key": "pypi", "member_repo_type": "local", "priority": 1}]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport),
            [
                format!("{API}/repositories/pypi-all"),
                format!("{API}/repositories/pypi-all/members")
            ]
        );
        let printed = outcome.json();
        assert_eq!(printed["id"], "pypi-all");
        assert_eq!(
            printed["members"],
            json!([{"id": "pypi", "type": "local", "priority": 1},
                {"id": "pypi-proxy", "type": "remote", "priority": 2}])
        );
        assert_eq!(printed["updated"], "2026-09-28T21:30:00Z");
    }

    #[test]
    fn a_proxy_says_how_long_it_keeps_upstream_answers() {
        let (outcome, transport) = anonymous(
            &["keeper", "repo", "get", "pypi-proxy"],
            vec![
                Answer::json(&repository("pypi-proxy", "pypi", "remote")),
                Answer::json(&json!({"repository_key": "pypi-proxy", "cache_ttl_seconds": 86400})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport)[1],
            format!("{API}/repositories/pypi-proxy/cache-ttl")
        );
        assert_eq!(outcome.json()["cache_ttl"], 86400);
        assert_eq!(outcome.json()["upstream"], "https://pypi.org");
    }

    #[test]
    fn a_missing_repository_is_exit_4_and_says_it_may_be_private() {
        let (outcome, _) = anonymous(
            &["keeper", "repo", "get", "internal"],
            vec![Answer::status(
                404,
                r#"{"code":"NOT_FOUND","message":"Repository not found"}"#,
            )],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("it is private"),
            "{}",
            outcome.stderr
        );
        assert!(outcome.stderr.contains("agent-cli keeper repo list"));
        let (outcome, _) = anonymous(
            &[
                "keeper",
                "repo",
                "get",
                "https://keeper.fabrikam.example/repositories/pypi",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
