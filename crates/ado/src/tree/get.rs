use std::collections::{BTreeMap, HashMap};

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Ado, Kind, list, text};
use crate::types;
use crate::work_items::{batch, person};

const CHILD: &str = "System.LinkTypes.Hierarchy-Forward";

/// Story points go by a different field in each process: Agile's Story
/// Points, Scrum's Effort, CMMI's Size. Every organization has all three.
const POINTS: [&str; 3] = [
    "Microsoft.VSTS.Scheduling.StoryPoints",
    "Microsoft.VSTS.Scheduling.Effort",
    "Microsoft.VSTS.Scheduling.Size",
];

const REMAINING: &str = "Microsoft.VSTS.Scheduling.RemainingWork";

const FIELDS: [&str; 9] = [
    "System.Id",
    "System.WorkItemType",
    "System.Title",
    "System.State",
    "System.AssignedTo",
    POINTS[0],
    POINTS[1],
    POINTS[2],
    REMAINING,
];

#[derive(clap::Args)]
pub struct TreeGetArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// Levels of children to print; deeper ones still count in the rollups
    #[arg(long, default_value_t = 5)]
    depth: usize,
}

/// A work item and everything under it, each level rolled up.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Node {
    id: i64,
    #[serde(rename = "type")]
    kind: Option<String>,
    title: Option<String>,
    state: Option<String>,
    assignee: Option<String>,
    points: Option<f64>,
    remaining_work: Option<f64>,
    /// Everything under this item, at any depth (not the item itself).
    rollup: Option<Rollup>,
    children: Vec<Node>,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
pub struct Rollup {
    items: usize,
    /// Items whose state is Completed or Removed for their type.
    done: usize,
    by_state: BTreeMap<String, usize>,
    points: f64,
    points_done: f64,
    remaining_work: f64,
    /// By points when there are any, else by items.
    percent_done: u32,
}

impl Rollup {
    fn add(&mut self, node: &Node, done: bool) {
        self.items += 1;
        self.done += usize::from(done);
        if let Some(state) = &node.state {
            *self.by_state.entry(state.clone()).or_default() += 1;
        }
        let points = node.points.unwrap_or_default();
        self.points += points;
        if done {
            self.points_done += points;
        }
        self.remaining_work += node.remaining_work.unwrap_or_default();
    }

    fn merge(&mut self, other: &Self) {
        self.items += other.items;
        self.done += other.done;
        for (state, count) in &other.by_state {
            *self.by_state.entry(state.clone()).or_default() += count;
        }
        self.points += other.points;
        self.points_done += other.points_done;
        self.remaining_work += other.remaining_work;
    }

    /// Works out `percent_done`, which `merge` leaves to each level.
    fn finish(&mut self) {
        let share = if self.points > 0.0 {
            self.points_done / self.points
        } else {
            self.done as f64 / self.items as f64
        };
        self.percent_done = (share * 100.0).round() as u32;
    }
}

fn node(item: &Value) -> Node {
    let fields = &item["fields"];
    Node {
        id: item["id"].as_i64().unwrap_or_default(),
        kind: text(&fields["System.WorkItemType"]),
        title: text(&fields["System.Title"]),
        state: text(&fields["System.State"]),
        assignee: person(&fields["System.AssignedTo"]),
        points: POINTS.iter().find_map(|field| fields[field].as_f64()),
        remaining_work: fields[REMAINING].as_f64(),
        rollup: None,
        children: Vec::new(),
    }
}

/// What a walk of the tree carries: the items read, who is under whom, the
/// done verdict per type and state (judged once each), and how many items
/// were counted below the printed depth.
struct Walk<'a> {
    ctx: &'a Ctx,
    ado: &'a Ado,
    items: HashMap<i64, &'a Value>,
    children: HashMap<i64, Vec<i64>>,
    done: HashMap<(String, String), bool>,
    max_depth: usize,
    hidden: usize,
}

impl Walk<'_> {
    fn done(&mut self, node: &Node) -> Result<bool> {
        let (Some(kind), Some(state)) = (&node.kind, &node.state) else {
            return Ok(false);
        };
        let key = (kind.clone(), state.clone());
        if let Some(done) = self.done.get(&key) {
            return Ok(*done);
        }
        let done = types::done(self.ctx, self.ado, kind, state)?;
        self.done.insert(key, done);
        Ok(done)
    }

    /// The node for `id` with its printed children, and the rollup of
    /// everything under it.
    fn build(&mut self, id: i64, depth: usize) -> Result<(Node, Rollup)> {
        let mut here = node(self.items[&id]);
        let mut rollup = Rollup::default();
        for child in self.children.get(&id).cloned().unwrap_or_default() {
            if !self.items.contains_key(&child) {
                continue;
            }
            let (node, below) = self.build(child, depth + 1)?;
            rollup.add(&node, self.done(&node)?);
            rollup.merge(&below);
            if depth < self.max_depth {
                here.children.push(node);
            } else {
                self.hidden += 1;
            }
        }
        if rollup.items > 0 {
            rollup.finish();
            here.rollup = Some(rollup.clone());
        }
        Ok((here, rollup))
    }
}

fn tree_get(ctx: &Ctx, args: TreeGetArgs) -> Result<Node> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let query = format!(
        "SELECT [System.Id] FROM WorkItemLinks WHERE [Source].[System.Id] = {id} \
         AND [System.Links.LinkType] = '{CHILD}' MODE (Recursive)"
    );
    let found = ado.query(ctx, &ado.work("wit/wiql", ""), json!({ "query": query }))?;
    let mut ids = vec![id];
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    for link in list(&found["workItemRelations"]) {
        let (Some(source), Some(target)) =
            (link["source"]["id"].as_i64(), link["target"]["id"].as_i64())
        else {
            continue;
        };
        children.entry(source).or_default().push(target);
        ids.push(target);
    }
    let read = batch(ctx, &ado, &ids, &FIELDS)?;
    let items: HashMap<i64, &Value> = read
        .iter()
        .filter_map(|item| Some((item["id"].as_i64()?, item)))
        .collect();
    if !items.contains_key(&id) {
        return Err(Failure::not_found(format!("work item {id} does not exist")).into());
    }
    let mut walk = Walk {
        ctx,
        ado: &ado,
        items,
        children,
        done: HashMap::new(),
        max_depth: args.depth,
        hidden: 0,
    };
    let (root, _) = walk.build(id, 0)?;
    if walk.hidden > 0 {
        ctx.note(format!(
            "[{} items below --depth {} are counted in the rollups, not shown]",
            walk.hidden, args.depth
        ));
    }
    Ok(root)
}

command! {
    pub TREE_GET = ["ado", "tree", "get"], Read,
    "Show a work item's hierarchy with progress rolled up: done, points, remaining",
    keywords: ["epic", "descendants", "progress", "percent", "rollup", "breakdown", "subtasks", "everything", "far", "along"],
    example: "ado tree get 42 --depth 2",
    run: tree_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testing::{BASE, ado, batch};

    fn work(
        id: i64,
        kind: &str,
        state: &str,
        points: Option<f64>,
        remaining: Option<f64>,
    ) -> Value {
        json!({"id": id, "fields": {
            "System.Id": id, "System.WorkItemType": kind, "System.Title": format!("w{id}"),
            "System.State": state,
            "Microsoft.VSTS.Scheduling.StoryPoints": points,
            "Microsoft.VSTS.Scheduling.RemainingWork": remaining
        }})
    }

    fn links(pairs: &[(i64, i64)]) -> Answer {
        let mut relations = vec![json!({"rel": null, "source": null, "target": {"id": 1}})];
        relations.extend(pairs.iter().map(|(source, target)| {
            json!({"rel": "System.LinkTypes.Hierarchy-Forward",
                "source": {"id": source}, "target": {"id": target}})
        }));
        Answer::json(&json!({"queryType": "tree", "workItemRelations": relations}))
    }

    fn states(names: &[(&str, &str)]) -> Answer {
        let value: Vec<Value> = names
            .iter()
            .map(|(name, category)| json!({"name": name, "category": category}))
            .collect();
        Answer::json(&json!({"value": value}))
    }

    fn story() -> Answer {
        states(&[
            ("New", "Proposed"),
            ("Active", "InProgress"),
            ("Closed", "Completed"),
        ])
    }

    fn task() -> Answer {
        states(&[("Active", "InProgress"), ("Done", "Completed")])
    }

    #[test]
    fn a_tree_rolls_up_done_points_and_remaining_work_by_category() {
        // 1 Feature > 2 Story (Closed, 3 points), 3 Story (Active, 5) > 4 Task (Done), 5 Task (Active, 6h)
        let (outcome, transport) = ado(
            &["ado", "tree", "get", "1"],
            vec![
                links(&[(1, 2), (1, 3), (3, 4), (3, 5)]),
                batch(vec![
                    work(3, "User Story", "Active", Some(5.0), None),
                    work(1, "Feature", "Active", None, None),
                    work(2, "User Story", "Closed", Some(3.0), None),
                    work(4, "Task", "Done", None, Some(0.0)),
                    work(5, "Task", "Active", None, Some(6.0)),
                ]),
                // One read per type and state judged, in walk order (2, 4, 5, 3);
                // on disk the states are cached a day.
                story(),
                task(),
                task(),
                story(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(
            got["rollup"],
            json!({"items": 4, "done": 2, "by_state": {"Active": 2, "Closed": 1, "Done": 1},
                "points": 8.0, "points_done": 3.0, "remaining_work": 6.0, "percent_done": 38})
        );
        assert_eq!(got["children"][0]["id"], 2);
        assert!(got["children"][0].get("rollup").is_none());
        assert_eq!(
            got["children"][1]["rollup"],
            json!({"items": 2, "done": 1, "by_state": {"Active": 1, "Done": 1},
                "points": 0.0, "points_done": 0.0, "remaining_work": 6.0, "percent_done": 50})
        );
        assert_eq!(got["children"][1]["children"][1]["remaining_work"], 6.0);
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            format!("{BASE}/Fabrikam/_apis/wit/wiql?api-version=7.1")
        );
        assert!(
            sent[0].body.as_ref().unwrap()["query"]
                .as_str()
                .unwrap()
                .contains("[Source].[System.Id] = 1 AND [System.Links.LinkType] = 'System.LinkTypes.Hierarchy-Forward' MODE (Recursive)")
        );
        assert_eq!(
            sent[1].body.as_ref().unwrap()["ids"],
            json!([1, 2, 3, 4, 5])
        );
        assert_eq!(sent.len(), 6, "each type and state is judged once");
    }

    #[test]
    fn depth_hides_deeper_children_but_counts_them() {
        let (outcome, _) = ado(
            &["ado", "tree", "get", "1", "--depth", "1"],
            vec![
                links(&[(1, 2), (2, 3)]),
                batch(vec![
                    work(1, "Epic", "Active", None, None),
                    work(2, "Epic", "Active", None, None),
                    work(3, "Epic", "Closed", None, None),
                ]),
                states(&[("Active", "InProgress"), ("Closed", "Completed")]),
                states(&[("Active", "InProgress"), ("Closed", "Completed")]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["rollup"]["items"], 2);
        assert_eq!(got["children"][0]["rollup"]["done"], 1);
        assert!(got["children"][0].get("children").is_none());
        assert!(
            outcome.stderr.contains("[1 items below --depth 1"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = ado(
            &["ado", "tree", "get", "1"],
            vec![links(&[]), batch(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
