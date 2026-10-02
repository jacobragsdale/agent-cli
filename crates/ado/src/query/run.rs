use std::collections::HashMap;

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, list};
use crate::ids::{arg, is_guid, query_param, web};
use crate::work_items::{WorkItemRow, rows};

use super::{QueryRow, queries};

#[derive(clap::Args)]
pub struct QueryRunArgs {
    /// The saved query: its id or path as query list prints them, a name no other query has, or its web URL
    query: String,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A work item the query found, as workitem list prints it; a tree or
/// one-hop query also says which row it hangs off.
#[derive(Debug, Serialize, JsonSchema)]
pub struct QueryResultRow {
    #[serde(flatten)]
    item: WorkItemRow,
    /// In a tree query, the work item this one sits under.
    parent: Option<i64>,
    /// In a one-hop query, the work item this one is linked from.
    linked_from: Option<i64>,
}

fn query_run(ctx: &Ctx, args: QueryRunArgs) -> Result<Vec<QueryResultRow>> {
    let ado = Ado::load(ctx)?;
    let id = resolve(ctx, &ado, &args.query)?;
    let path = format!("wit/wiql/{id}");
    let top = format!("$top={}", args.limit + 1);
    // A team's query can say @CurrentIteration, which only a team answers.
    let url = match ado.teams.as_slice() {
        [team] => ado.team(team, &path, &top),
        _ => ado.work(&path, &top),
    };
    let answer = ado.get(ctx, &url)?;
    // (work item, the one it hangs off): a link query answers links, whose
    // roots have no source; a flat one answers work items.
    let mut found: Vec<(i64, Option<i64>)> = if answer["workItemRelations"].is_array() {
        list(&answer["workItemRelations"])
            .iter()
            .filter_map(|link| {
                Some((
                    link["target"]["id"].as_i64()?,
                    link["source"]["id"].as_i64(),
                ))
            })
            .collect()
    } else {
        list(&answer["workItems"])
            .iter()
            .filter_map(|item| Some((item["id"].as_i64()?, None)))
            .collect()
    };
    if found.len() > args.limit {
        found.truncate(args.limit);
        ctx.note(format!("[first {}; --limit N for more]", args.limit));
    }
    let mut ids: Vec<i64> = Vec::with_capacity(found.len());
    for (id, _) in &found {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    let read: HashMap<i64, WorkItemRow> = rows(ctx, &ado, &ids)?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let tree = answer["queryType"].as_str() == Some("tree");
    Ok(found
        .into_iter()
        .filter_map(|(id, from)| {
            Some(QueryResultRow {
                item: read.get(&id)?.clone(),
                parent: from.filter(|_| tree),
                linked_from: from.filter(|_| !tree),
            })
        })
        .collect())
}

/// The query's GUID from whatever names it. A URL is read, never fetched:
/// `…/_queries/query/{id}/`, `…/query-edit/{id}` or `qr.aspx?qid={id}`.
fn resolve(ctx: &Ctx, ado: &Ado, raw: &str) -> Result<String> {
    let raw = raw.trim();
    if is_guid(raw) {
        return Ok(raw.to_owned());
    }
    if let Some((segments, query)) = web(ado, raw).map_err(Failure::usage)? {
        return segments
            .iter()
            .position(|segment| segment == "query" || segment == "query-edit")
            .and_then(|at| segments.get(at + 1).cloned())
            .or_else(|| query_param(&query, "qid"))
            .filter(|id| is_guid(id))
            .ok_or_else(|| {
                Failure::usage(format!("{raw} is not a saved query's URL"))
                    .hint("agent-cli ado query list --fields id,path")
                    .into()
            });
    }
    let all = queries(ctx, ado)?;
    let wanted = raw.trim_matches('/');
    if let Some(query) = all.iter().find(|q| q.path.eq_ignore_ascii_case(wanted)) {
        return Ok(query.id.clone());
    }
    let named: Vec<&QueryRow> = all
        .iter()
        .filter(|q| q.name.eq_ignore_ascii_case(wanted))
        .collect();
    match named.as_slice() {
        [query] => Ok(query.id.clone()),
        [] => Err(
            Failure::not_found(format!("no saved query is named {raw} or has that path"))
                .hint("agent-cli ado query list --text TEXT")
                .into(),
        ),
        several => {
            let paths: Vec<&str> = several.iter().map(|q| q.path.as_str()).collect();
            Err(Failure::usage(format!(
                "{} saved queries are named {raw}: {}; pass the path or the id",
                paths.len(),
                paths.join(", ")
            ))
            .hint(format!("agent-cli ado query list --text {}", arg(raw)))
            .into())
        }
    }
}

command! {
    pub QUERY_RUN = ["ado", "query", "run"], Read,
    "Run a saved work item query (shared or my query) and list the work items",
    keywords: ["saved", "shared", "triage", "results", "execute", "wiql", "tickets", "named"],
    example: "ado query run 'Shared Queries/Triage' --fields id,title,state,assignee",
    run: query_run,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::super::tests::{STORIES, TRIAGE, folder, tree};
    use crate::testing::{BASE, ado, batch, item, urls, wiql};

    #[test]
    fn a_flat_query_by_path_prints_workitem_list_rows_through_the_one_team() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "query",
                "run",
                "shared queries/triage",
                "--fields",
                "id,title,parent",
            ],
            vec![
                tree(),
                folder(),
                wiql(&[12, 11]),
                batch(vec![item(11, 1, "Eleven"), item(12, 3, "Twelve")]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": 12, "title": "Twelve"}, {"id": 11, "title": "Eleven"}])
        );
        assert_eq!(
            urls(&transport)[2],
            format!("{BASE}/Fabrikam/Web%20Team/_apis/wit/wiql/{TRIAGE}?$top=51&api-version=7.1")
        );
    }

    #[test]
    fn a_tree_query_by_id_gives_each_child_its_parent_and_keeps_the_limit() {
        let link = |source: Option<i64>, target: i64| match source {
            Some(source) => json!({"rel": "System.LinkTypes.Hierarchy-Forward",
                "source": {"id": source}, "target": {"id": target}}),
            None => json!({"target": {"id": target}}),
        };
        let answer = Answer::json(&json!({"queryType": "tree", "workItemRelations": [
            link(None, 7), link(Some(7), 8), link(Some(7), 9)
        ]}));
        let (outcome, transport) = ado(
            &[
                "ado",
                "query",
                "run",
                STORIES,
                "--limit",
                "2",
                "--fields",
                "id,parent,linked_from",
            ],
            vec![answer, batch(vec![item(7, 1, "Story"), item(8, 1, "Task")])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!([{"id": 7}, {"id": 8, "parent": 7}]));
        assert!(outcome.stderr.contains("[first 2; --limit N for more]"));
        assert_eq!(
            transport.sent()[1].body.as_ref().unwrap()["ids"],
            json!([7, 8])
        );
    }

    #[test]
    fn a_one_hop_query_names_what_each_row_is_linked_from_and_reads_a_twice_linked_item_once() {
        let answer = Answer::json(&json!({"queryType": "oneHop", "workItemRelations": [
            {"target": {"id": 1}},
            {"rel": "System.LinkTypes.Related", "source": {"id": 1}, "target": {"id": 3}},
            {"target": {"id": 2}},
            {"rel": "System.LinkTypes.Related", "source": {"id": 2}, "target": {"id": 3}},
        ]}));
        let (outcome, transport) = ado(
            &["ado", "query", "run", STORIES, "--fields", "id,linked_from"],
            vec![
                answer,
                batch(vec![item(1, 1, "a"), item(2, 1, "b"), item(3, 1, "c")]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": 1}, {"id": 3, "linked_from": 1}, {"id": 2}, {"id": 3, "linked_from": 2}])
        );
        assert_eq!(
            transport.sent()[1].body.as_ref().unwrap()["ids"],
            json!([1, 3, 2])
        );
    }

    #[test]
    fn a_web_url_names_the_query_and_another_orgs_is_exit_2() {
        let url = format!("https://dev.azure.com/contoso/Fabrikam/_queries/query/{STORIES}/");
        let (outcome, transport) = ado(&["ado", "query", "run", &url], vec![wiql(&[])]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(urls(&transport)[0].contains(&format!("/wit/wiql/{STORIES}?")));

        let (outcome, transport) = ado(
            &[
                "ado",
                "query",
                "run",
                "https://dev.azure.com/other/P/_queries/query/x",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn a_name_two_queries_share_is_exit_2_and_no_query_is_exit_4() {
        let (outcome, _) = ado(&["ado", "query", "run", "Triage"], vec![tree(), folder()]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("Shared Queries/Triage, Shared Queries/Web Team/Triage"),
            "{}",
            outcome.stderr
        );

        let (outcome, transport) = ado(
            &["ado", "query", "run", "stories WITH tasks"],
            vec![tree(), folder(), wiql(&[])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(urls(&transport)[2].contains(&format!("/wit/wiql/{STORIES}?")));

        let (outcome, _) = ado(&["ado", "query", "run", "Nightly"], vec![tree(), folder()]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
