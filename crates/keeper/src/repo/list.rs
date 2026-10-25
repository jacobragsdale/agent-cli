//! `keeper repo list`: `GET /repositories`, filtered by Keeper and paged by
//! number.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;

use super::RepoRow;
use crate::client::{Keeper, note_more};

#[derive(clap::Args)]
pub struct RepoListArgs {
    /// Only repositories whose key or name contains this
    name: Option<String>,
    /// Only this format: pypi, generic, npm, maven, docker, cargo, nuget …
    #[arg(long)]
    format: Option<String>,
    /// Only hosted (local), proxy (remote) or virtual repositories
    #[arg(long = "type", value_parser = PossibleValuesParser::new(["local", "remote", "virtual"]))]
    kind: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn repo_list(ctx: &Ctx, args: RepoListArgs) -> Result<Vec<RepoRow>> {
    let keeper = Keeper::load(ctx)?;
    let mut query = Vec::new();
    if let Some(name) = args.name {
        query.push(("q", name));
    }
    if let Some(format) = args.format {
        query.push(("format", format.to_ascii_lowercase()));
    }
    if let Some(kind) = args.kind {
        query.push(("type", kind));
    }
    let (found, total) = keeper.pages(ctx, "/repositories", &query, args.limit)?;
    let mut rows: Vec<RepoRow> = found.iter().map(RepoRow::of).collect();
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        note_more(ctx, args.limit, total);
    }
    if rows.is_empty() && !keeper.signed_in() {
        ctx.note(
            "[none public; private repositories need a token under [keeper]: agent-cli doctor keeper]",
        );
    }
    Ok(rows)
}

command! {
    pub REPO_LIST = ["keeper", "repo", "list"], Read,
    "List Artifact Keeper repositories: PyPI, generic, proxies of upstream, virtual",
    keywords: ["feeds", "indexes", "hosted", "proxy", "mirror", "pypi", "generic", "artifactory", "nexus"],
    example: "keeper repo list --format pypi --fields id,type,upstream",
    run: repo_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{API, TOKEN, anonymous, keeper_run, page, repository, urls};

    #[test]
    fn repositories_list_with_their_type_and_upstream_and_say_when_cut() {
        let (outcome, transport) = keeper_run(
            &[
                "keeper", "repo", "list", "py", "--type", "remote", "--limit", "1",
            ],
            vec![page(
                vec![
                    repository("pypi-proxy", "pypi", "remote"),
                    repository("pypi-old", "pypi", "remote"),
                ],
                2,
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "pypi-proxy", "format": "pypi", "type": "remote", "visibility": "public",
                "upstream": "https://pypi.org", "size": 1_048_576, "created": "2026-09-01T09:00:00Z"}])
        );
        assert_eq!(
            urls(&transport),
            [format!(
                "{API}/repositories?q=py&type=remote&page=1&per_page=2"
            )]
        );
        assert!(
            outcome.stderr.contains("[1 of 2; --limit N]"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent()[0].authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
    }

    #[test]
    fn anonymous_lists_send_no_token_and_name_the_way_to_private_ones() {
        let (outcome, transport) = anonymous(&["keeper", "repo", "list"], vec![page(vec![], 0)]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(transport.sent()[0].authorization, None);
        assert!(
            outcome.stderr.contains("need a token"),
            "{}",
            outcome.stderr
        );
    }
}
