use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, list, query_value, short_branch, stamp, text};

#[derive(clap::Args)]
pub struct PipelineListArgs {
    /// Only names containing this
    pattern: Option<String>,
    /// Only pipelines that build this repository
    #[arg(long)]
    repo: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PipelineRow {
    id: i64,
    name: Option<String>,
    folder: Option<String>,
    /// enabled, paused or disabled.
    queue_status: Option<String>,
    last_run: Option<LastRun>,
    url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LastRun {
    id: i64,
    status: Option<String>,
    result: Option<String>,
    branch: Option<String>,
    finished: Option<String>,
}

fn pipeline_list(ctx: &Ctx, args: PipelineListArgs) -> Result<Vec<PipelineRow>> {
    let ado = Ado::load(ctx)?;
    let mut query = format!(
        "includeLatestBuilds=true&queryOrder=definitionNameAscending&$top={}",
        args.limit + 1
    );
    if let Some(pattern) = &args.pattern {
        query.push_str(&format!(
            "&name={}",
            query_value(&format!("*{}*", pattern.trim()))
        ));
    }
    if let Some(repo) = &args.repo {
        let repo = ado.repo(ctx, repo)?;
        query.push_str(&format!("&repositoryId={}&repositoryType=TfsGit", repo.id));
    }
    let answer = ado.get(ctx, &ado.code("build/definitions", &query))?;
    let mut rows: Vec<PipelineRow> = list(&answer["value"])
        .iter()
        .filter_map(|definition| {
            let latest = &definition["latestBuild"];
            Some(PipelineRow {
                id: definition["id"].as_i64()?,
                name: text(&definition["name"]),
                folder: text(&definition["path"]),
                queue_status: text(&definition["queueStatus"]),
                last_run: latest["id"].as_i64().map(|id| LastRun {
                    id,
                    status: text(&latest["status"]),
                    result: text(&latest["result"]),
                    branch: latest["sourceBranch"].as_str().map(short_branch),
                    finished: stamp(&latest["finishTime"]),
                }),
                url: text(&definition["_links"]["web"]["href"]),
            })
        })
        .collect();
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!(
            "[first {}; --limit N, or narrow with PATTERN or --repo]",
            args.limit
        ));
    }
    Ok(rows)
}

command! {
    pub PIPELINE_LIST = ["ado", "pipeline", "list"], Read,
    "List build pipelines, each with its last result",
    keywords: ["definitions", "ci", "workflows", "find"],
    example: "ado pipeline list --repo web --fields id,name,last_run.result",
    run: pipeline_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{CODE, ado, page, urls};

    #[test]
    fn pipeline_list_filters_by_repo_and_shows_the_last_run() {
        let (outcome, transport) = ado(
            &[
                "ado", "pipeline", "list", "web", "--repo", "web", "--limit", "1",
            ],
            vec![
                page(vec![
                    json!({"id": "r-1", "name": "web", "project": {"id": "p-1"}}),
                ]),
                page(vec![
                    json!({"id": 12, "name": "web-ci", "path": "\\", "queueStatus": "enabled",
                        "latestBuild": {"id": 991, "status": "completed", "result": "failed",
                            "sourceBranch": "refs/heads/main", "finishTime": "2026-09-29T10:09:00Z"},
                        "_links": {"web": {"href": "https://dev.azure.com/contoso/Fabrikam/_build/definition?definitionId=12"}}}),
                    json!({"id": 13, "name": "web-nightly"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0]["last_run"],
            json!({"id": 991, "status": "completed", "result": "failed", "branch": "main",
                "finished": "2026-09-29T10:09:00Z"})
        );
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert!(
            outcome.stderr.starts_with("[first 1;"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{CODE}/build/definitions?includeLatestBuilds=true&queryOrder=definitionNameAscending&$top=2&name=%2Aweb%2A&repositoryId=r-1&repositoryType=TfsGit&api-version=7.1"
            )
        );
    }
}
