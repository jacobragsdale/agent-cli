use agent_cli_core::{Ctx, command};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, list, segment, short_branch, text};

#[derive(clap::Args)]
pub struct RepoGetArgs {
    /// The repository's name or id
    name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Repo {
    name: String,
    id: String,
    project: Option<String>,
    default_branch: Option<String>,
    size: Option<i64>,
    is_disabled: Option<bool>,
    remote_url: Option<String>,
    ssh_url: Option<String>,
    web_url: Option<String>,
    /// Up to 100, by name.
    branches: Vec<String>,
}

/// At most this many branches are listed; a repository with more is one to
/// search rather than read.
const BRANCHES: usize = 100;

fn repo_get(ctx: &Ctx, args: RepoGetArgs) -> Result<Repo> {
    let ado = Ado::load(ctx)?;
    let repo = ado.get(
        ctx,
        &ado.code(&format!("git/repositories/{}", segment(&args.name)), ""),
    )?;
    let id = text(&repo["id"]).context("the repository came back without an id")?;
    let refs = ado.get(
        ctx,
        &ado.code(
            &format!("git/repositories/{id}/refs"),
            &format!("filter=heads/&$top={}", BRANCHES + 1),
        ),
    )?;
    let mut branches: Vec<String> = list(&refs["value"])
        .iter()
        .filter_map(|entry| entry["name"].as_str().map(short_branch))
        .collect();
    if branches.len() > BRANCHES {
        branches.truncate(BRANCHES);
        ctx.note(format!("[first {BRANCHES} branches]"));
    }
    Ok(Repo {
        name: text(&repo["name"]).unwrap_or_default(),
        id,
        project: text(&repo["project"]["name"]),
        default_branch: repo["defaultBranch"].as_str().map(short_branch),
        size: repo["size"].as_i64(),
        is_disabled: repo["isDisabled"].as_bool().filter(|disabled| *disabled),
        remote_url: text(&repo["remoteUrl"]),
        ssh_url: text(&repo["sshUrl"]),
        web_url: text(&repo["webUrl"]),
        branches,
    })
}

command! {
    pub REPO_GET = ["ado", "repo", "get"], Read,
    "Show a repository: its URLs, default branch and branches",
    keywords: ["repository", "clone", "url", "branches", "remote"],
    example: "ado repo get web --fields remote_url,default_branch",
    run: repo_get,
}
