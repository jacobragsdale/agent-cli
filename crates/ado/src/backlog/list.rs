use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, list, segment, text};
use crate::iteration::team;
use crate::work_items::{POINTS, ROW_FIELDS, WorkItemRow, points, read, row};

#[derive(clap::Args)]
pub struct BacklogListArgs {
    /// stories (the team's requirement backlog), features or epics
    #[arg(long, default_value = "stories", value_parser = PossibleValuesParser::new(["stories", "features", "epics"]))]
    level: String,
    /// The team whose backlog it is (default: [ado] team)
    #[arg(long)]
    team: Option<String>,
    /// Most rows to return (from the top)
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BacklogRow {
    #[serde(flatten)]
    item: WorkItemRow,
    /// Its place on the backlog, from 1 at the top.
    rank: usize,
    /// Story points, effort or size, whichever the process uses.
    points: Option<f64>,
}

fn backlog_list(ctx: &Ctx, args: BacklogListArgs) -> Result<Vec<BacklogRow>> {
    let ado = Ado::load(ctx)?;
    let team = team(&ado, args.team.as_deref())?;
    let levels = ado.get(ctx, &ado.team(team, "work/backlogs", ""))?;
    // Processes name the requirement level differently (Stories, Backlog
    // items, Requirements, Issues), so it is found by its type.
    let level = list(&levels["value"])
        .iter()
        .find(|level| match args.level.as_str() {
            "features" => level["id"] == "Microsoft.FeatureCategory",
            "epics" => level["id"] == "Microsoft.EpicCategory",
            _ => level["type"] == "requirement",
        });
    let Some(id) = level.and_then(|level| text(&level["id"])) else {
        let names: Vec<String> = list(&levels["value"])
            .iter()
            .filter_map(|level| text(&level["name"]))
            .collect();
        return Err(Failure::not_found(format!(
            "{team} has no {} backlog; its backlogs are: {}",
            args.level,
            names.join(", ")
        ))
        .into());
    };
    // The order is the backlog's own (its hidden backlog priority), which
    // WIQL cannot sort by reliably.
    let url = ado.team(
        team,
        &format!("work/backlogs/{}/workItems", segment(&id)),
        "",
    );
    let answer = ado.get(ctx, &url)?;
    let mut ids: Vec<i64> = Vec::new();
    for id in list(&answer["workItems"])
        .iter()
        .filter_map(|link| link["target"]["id"].as_i64())
    {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.len() > args.limit {
        ctx.note(format!(
            "[the top {} of {}; --limit N for more]",
            args.limit,
            ids.len()
        ));
        ids.truncate(args.limit);
    }
    let mut fields = ROW_FIELDS.to_vec();
    fields.extend(POINTS);
    let items = read(ctx, &ado, &ids, &fields)?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let at = ids.iter().position(|id| item["id"].as_i64() == Some(*id))?;
            Some(BacklogRow {
                item: row(item),
                rank: at + 1,
                points: points(item),
            })
        })
        .collect())
}

command! {
    pub BACKLOG_LIST = ["ado", "backlog", "list"], Read,
    "List a team's ranked backlog (stories, features or epics) with points",
    keywords: ["ranked", "ranking", "priority", "prioritized", "top", "stories", "features", "epics", "product", "groomed", "refinement"],
    example: "ado backlog list --level stories --limit 20 --fields rank,id,title,points",
    run: backlog_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, batch, item, urls};

    fn levels() -> Answer {
        Answer::json(&json!({"count": 3, "value": [
            {"id": "Microsoft.EpicCategory", "name": "Epics", "rank": 3, "type": "portfolio"},
            {"id": "Microsoft.FeatureCategory", "name": "Features", "rank": 2, "type": "portfolio"},
            {"id": "Microsoft.RequirementCategory", "name": "Backlog items", "rank": 1, "type": "requirement"}
        ]}))
    }

    #[test]
    fn the_backlog_keeps_its_own_order_with_a_rank_and_points() {
        let mut first = item(30, 3, "Pay by card");
        first["fields"]["Microsoft.VSTS.Scheduling.Effort"] = json!(5);
        let (outcome, transport) = ado(
            &["ado", "backlog", "list", "--limit", "2"],
            vec![
                levels(),
                Answer::json(&json!({"workItems": [
                    {"rel": null, "source": null, "target": {"id": 30}},
                    {"rel": null, "source": null, "target": {"id": 10}},
                    {"rel": null, "source": null, "target": {"id": 20}}
                ]})),
                batch(vec![item(10, 1, "Second"), first]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["id"], 30);
        assert_eq!(rows[0]["rank"], 1);
        assert_eq!(rows[0]["points"], 5.0);
        assert_eq!(rows[1]["id"], 10);
        assert_eq!(rows[1]["rank"], 2);
        assert_eq!(rows[1].get("points"), None);
        assert_eq!(outcome.stderr, "[the top 2 of 3; --limit N for more]\n");
        let team = format!("{BASE}/Fabrikam/Web%20Team/_apis/work/backlogs");
        assert_eq!(
            urls(&transport)[..2],
            [
                format!("{team}?api-version=7.1"),
                format!("{team}/Microsoft.RequirementCategory/workItems?api-version=7.1"),
            ]
        );
        let batch = transport.sent()[2].body.clone().unwrap();
        assert!(
            batch["fields"]
                .as_array()
                .unwrap()
                .contains(&json!("Microsoft.VSTS.Scheduling.StoryPoints")),
            "{batch}"
        );
        assert_eq!(batch["ids"], json!([30, 10]));
    }

    #[test]
    fn a_portfolio_level_is_found_by_its_category_and_a_missing_one_is_not_found() {
        let (outcome, transport) = ado(
            &["ado", "backlog", "list", "--level", "features"],
            vec![levels(), Answer::json(&json!({"workItems": []}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!([]));
        assert!(urls(&transport)[1].contains("/backlogs/Microsoft.FeatureCategory/workItems"));

        let (outcome, _) = ado(
            &["ado", "backlog", "list", "--level", "epics"],
            vec![Answer::json(&json!({"value": [
                {"id": "Microsoft.RequirementCategory", "name": "Stories", "type": "requirement"}
            ]}))],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("its backlogs are: Stories"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = ado(&["ado", "backlog", "list", "--level", "tasks"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
