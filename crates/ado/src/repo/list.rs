use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, list, short_branch, text};

#[derive(clap::Args)]
pub struct RepoListArgs {
    /// Only names containing this
    pattern: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RepoRow {
    name: String,
    id: String,
    default_branch: Option<String>,
    /// Bytes.
    size: Option<i64>,
    is_disabled: Option<bool>,
    web_url: Option<String>,
}

fn repo_list(ctx: &Ctx, args: RepoListArgs) -> Result<Vec<RepoRow>> {
    let ado = Ado::load(ctx)?;
    let answer = ado.get(ctx, &ado.code("git/repositories", ""))?;
    let pattern = args.pattern.as_deref().unwrap_or_default().to_lowercase();
    let mut repos: Vec<RepoRow> = list(&answer["value"])
        .iter()
        .filter_map(|repo| {
            Some(RepoRow {
                name: text(&repo["name"])?,
                id: text(&repo["id"])?,
                default_branch: repo["defaultBranch"].as_str().map(short_branch),
                size: repo["size"].as_i64(),
                is_disabled: repo["isDisabled"].as_bool().filter(|disabled| *disabled),
                web_url: text(&repo["webUrl"]),
            })
        })
        .filter(|repo| repo.name.to_lowercase().contains(&pattern))
        .collect();
    repos.sort_by_key(|repo| repo.name.to_lowercase());
    if repos.len() > args.limit {
        ctx.note(format!(
            "[{} of {}; --limit N, or narrow with PATTERN]",
            args.limit,
            repos.len()
        ));
        repos.truncate(args.limit);
    }
    Ok(repos)
}

command! {
    pub REPO_LIST = ["ado", "repo", "list"], Read,
    "List the project's Git repositories",
    keywords: ["repositories", "git", "code", "projects", "find"],
    example: "ado repo list web --fields name,default_branch",
    run: repo_list,
}
