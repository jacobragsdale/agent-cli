use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::Ado;

use super::{QueryRow, queries};

#[derive(clap::Args)]
pub struct QueryListArgs {
    /// Only queries whose name or path holds this text (any case)
    #[arg(long)]
    text: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
    /// The project (default: the first in [ado] project)
    #[arg(long)]
    project: Option<String>,
}

fn query_list(ctx: &Ctx, args: QueryListArgs) -> Result<Vec<QueryRow>> {
    let ado = Ado::load_in(ctx, args.project.as_deref())?;
    let wanted = args.text.as_deref().map(str::to_lowercase);
    let mut rows: Vec<QueryRow> = queries(ctx, &ado)?
        .into_iter()
        .filter(|row| {
            wanted.as_deref().is_none_or(|wanted| {
                row.name.to_lowercase().contains(wanted) || row.path.to_lowercase().contains(wanted)
            })
        })
        .collect();
    if rows.len() > args.limit {
        ctx.note(format!(
            "[{} of {}; --limit N, or --text TEXT to narrow]",
            args.limit,
            rows.len()
        ));
        rows.truncate(args.limit);
    }
    Ok(rows)
}

command! {
    pub QUERY_LIST = ["ado", "query", "list"], Read,
    "List saved work item queries in My Queries and Shared Queries",
    keywords: ["saved", "shared", "favorite", "folder", "wiql", "triage", "find", "named"],
    example: "ado query list --text triage --fields id,path,type",
    run: query_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::tests::{DEEP, MINE, STORIES, TRIAGE, folder, tree};
    use crate::testing::{BASE, ado, urls};

    #[test]
    fn list_walks_both_folders_and_reads_on_where_the_two_level_walk_stopped() {
        let (outcome, transport) = ado(&["ado", "query", "list"], vec![tree(), folder()]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": MINE, "name": "Assigned to me", "path": "My Queries/Assigned to me",
                 "type": "flat", "is_public": false},
                {"id": TRIAGE, "name": "Triage", "path": "Shared Queries/Triage",
                 "type": "flat", "is_public": true},
                {"id": STORIES, "name": "Stories with tasks",
                 "path": "Shared Queries/Web Team/Stories with tasks", "type": "tree",
                 "is_public": true},
                {"id": DEEP, "name": "Triage", "path": "Shared Queries/Web Team/Triage",
                 "type": "flat", "is_public": true},
            ])
        );
        assert_eq!(
            urls(&transport),
            [
                format!(
                    "{BASE}/Fabrikam/_apis/wit/queries?$depth=2&$expand=minimal&api-version=7.1"
                ),
                format!(
                    "{BASE}/Fabrikam/_apis/wit/queries/f0000000-0000-4000-8000-000000000003?$depth=2&$expand=minimal&api-version=7.1"
                ),
            ]
        );
    }

    #[test]
    fn text_narrows_by_name_or_path_and_the_limit_says_what_it_cut() {
        let (outcome, _) = ado(
            &["ado", "query", "list", "--text", "WEB TEAM", "--limit", "1"],
            vec![tree(), folder()],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["id"], STORIES);
        assert_eq!(outcome.json().as_array().unwrap().len(), 1);
        assert!(
            outcome
                .stderr
                .contains("[1 of 2; --limit N, or --text TEXT to narrow]"),
            "{}",
            outcome.stderr
        );
    }
}
