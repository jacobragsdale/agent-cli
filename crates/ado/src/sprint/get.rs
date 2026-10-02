use std::collections::{BTreeMap, HashMap};

use agent_cli_core::{Ctx, When, command, now};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Ado, list, segment, stamp, text};
use crate::ids::arg;
use crate::iteration::{resolve, team};
use crate::work_items::{POINTS, points};

use super::{STATE, SprintRow, done, items};

const ASSIGNED: &str = "System.AssignedTo";
const REMAINING: &str = "Microsoft.VSTS.Scheduling.RemainingWork";

#[derive(clap::Args)]
pub struct SprintGetArgs {
    /// @current, @next, @previous, a sprint's path or its name
    #[arg(default_value = "@current")]
    sprint: String,
    /// The team whose sprint it is (default: [ado] team)
    #[arg(long)]
    team: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SprintDetail {
    #[serde(flatten)]
    sprint: SprintRow,
    /// Weekdays from today (or the start) to the finish, less team days off.
    working_days_left: usize,
    totals: Totals,
    people: Vec<Load>,
    team_days_off: Vec<DaysOff>,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct Totals {
    items: usize,
    by_state: BTreeMap<String, usize>,
    points: f64,
    points_done: f64,
    /// Hours left on unfinished work.
    remaining_work: f64,
}

/// One person's share of the sprint against their capacity.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Load {
    name: String,
    items: usize,
    points: f64,
    remaining_work: f64,
    /// Hours a day, over all their activities; absent when not planned.
    capacity_per_day: Option<f64>,
    /// Their own working days off still ahead in the sprint.
    days_off: Option<usize>,
    /// Hours they have left: capacity a day times their working days left.
    capacity_left: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DaysOff {
    start: Option<String>,
    end: Option<String>,
}

/// A person's load while it is added up, under their sign-in address.
struct Person {
    load: Load,
    address: String,
}

fn sprint_get(ctx: &Ctx, args: SprintGetArgs) -> Result<SprintDetail> {
    let ado = Ado::load(ctx)?;
    let team = team(&ado, args.team.as_deref())?;
    let sprint = resolve(ctx, &ado, Some(team), &args.sprint)?;
    let mut fields = vec![ASSIGNED, REMAINING];
    fields.extend(POINTS);
    let (work, _) = items(ctx, &ado, team, &sprint, &fields)?;

    let mut totals = Totals::default();
    let mut people: Vec<Person> = Vec::new();
    let mut seen = HashMap::new();
    for item in &work {
        let finished = done(ctx, &ado, &mut seen, item)?;
        let state = text(&item["fields"][STATE]).unwrap_or_default();
        let points = points(item).unwrap_or_default();
        let remaining = if finished {
            0.0
        } else {
            item["fields"][REMAINING].as_f64().unwrap_or_default()
        };
        totals.items += 1;
        *totals.by_state.entry(state).or_default() += 1;
        totals.points += points;
        if finished {
            totals.points_done += points;
        }
        totals.remaining_work += remaining;
        let assigned = &item["fields"][ASSIGNED];
        if let Some(address) = text(&assigned["uniqueName"]) {
            let name = text(&assigned["displayName"]).unwrap_or_else(|| address.clone());
            let person = person(&mut people, &address, name);
            person.load.items += 1;
            person.load.points += points;
            person.load.remaining_work += remaining;
        }
    }

    let iteration = format!("work/teamsettings/iterations/{}", segment(&sprint.id));
    let capacities = ado.get(ctx, &ado.team(team, &format!("{iteration}/capacities"), ""))?;
    let team_days_off = ado.get(
        ctx,
        &ado.team(team, &format!("{iteration}/teamdaysoff"), ""),
    )?;
    let team_days_off = list(&team_days_off["daysOff"]);
    let team_off = ranges(team_days_off);
    let days_left = match (
        sprint.start.as_deref().and_then(day),
        sprint.finish.as_deref().and_then(day),
    ) {
        (Some(start), Some(finish)) => working_days(start.max(day_now()), finish, &team_off),
        _ => Vec::new(),
    };
    for member in list(&capacities["teamMembers"]) {
        let identity = &member["teamMember"];
        let Some(address) = text(&identity["uniqueName"]) else {
            continue;
        };
        let name = text(&identity["displayName"]).unwrap_or_else(|| address.clone());
        let per_day: f64 = list(&member["activities"])
            .iter()
            .filter_map(|activity| activity["capacityPerDay"].as_f64())
            .sum();
        let off = ranges(list(&member["daysOff"]));
        let days_off = days_left.iter().filter(|d| off_on(&off, **d)).count();
        let load = &mut person(&mut people, &address, name).load;
        load.capacity_per_day = Some(per_day);
        load.days_off = Some(days_off);
        load.capacity_left = Some(per_day * (days_left.len() - days_off) as f64);
    }
    people.sort_by(|a, b| a.load.name.cmp(&b.load.name));

    // A past sprint has no time left for anyone; what it needs is closing.
    if sprint.timeframe.as_deref() != Some("past") {
        let over = |p: &&Person| match (p.load.capacity_per_day, p.load.capacity_left) {
            (Some(per_day), Some(left)) if per_day > 0.0 => p.load.remaining_work - left,
            _ => 0.0,
        };
        if let Some(most) = people
            .iter()
            .filter(|p| over(p) > 0.0)
            .max_by(|a, b| over(a).total_cmp(&over(b)))
        {
            ctx.note(format!(
                "[next: agent-cli ado workitem list --iteration {} --assignee {} ({} has {}h of work left and {}h of capacity)]",
                arg(&sprint.path),
                arg(&most.address),
                most.load.name,
                most.load.remaining_work,
                most.load.capacity_left.unwrap_or_default(),
            ));
        }
    }

    Ok(SprintDetail {
        working_days_left: days_left.len(),
        totals,
        people: people.into_iter().map(|p| p.load).collect(),
        team_days_off: team_days_off
            .iter()
            .map(|range| DaysOff {
                start: stamp(&range["start"]),
                end: stamp(&range["end"]),
            })
            .collect(),
        sprint: sprint.into(),
    })
}

command! {
    pub SPRINT_GET = ["ado", "sprint", "get"], Read,
    "Show how a sprint is going: totals, each person's load and capacity left",
    keywords: ["iteration", "progress", "status", "burndown", "capacity", "load", "overloaded", "remaining", "points", "velocity", "days", "off"],
    example: "ado sprint get @current --fields name,working_days_left,totals,people",
    run: sprint_get,
}

/// The entry for `address` in `people`, added under `name` when new.
fn person<'a>(people: &'a mut Vec<Person>, address: &str, name: String) -> &'a mut Person {
    let at = match people
        .iter()
        .position(|p| p.address.eq_ignore_ascii_case(address))
    {
        Some(at) => at,
        None => {
            people.push(Person {
                address: address.to_owned(),
                load: Load {
                    name,
                    items: 0,
                    points: 0.0,
                    remaining_work: 0.0,
                    capacity_per_day: None,
                    days_off: None,
                    capacity_left: None,
                },
            });
            people.len() - 1
        }
    };
    &mut people[at]
}

/// Days since 1970-01-01 (UTC) of a timestamp. Azure DevOps dates a sprint
/// and a day off at midnight UTC of the calendar day.
fn day(stamp: &str) -> Option<i64> {
    stamp
        .parse::<When>()
        .ok()
        .map(|when| when.unix().div_euclid(86_400))
}

fn day_now() -> i64 {
    now().unix_timestamp().div_euclid(86_400)
}

/// Days-off ranges as inclusive day spans.
fn ranges(days_off: &[Value]) -> Vec<(i64, i64)> {
    days_off
        .iter()
        .filter_map(|range| {
            let start = day(range["start"].as_str()?)?;
            Some((start, range["end"].as_str().and_then(day).unwrap_or(start)))
        })
        .collect()
}

fn off_on(off: &[(i64, i64)], day: i64) -> bool {
    off.iter()
        .any(|(start, end)| (*start..=*end).contains(&day))
}

/// The weekdays from `from` to `to`, both included, that are not off.
// ponytail: Monday to Friday; a team whose working days differ needs
// teamsettings' workingDays read here.
fn working_days(from: i64, to: i64, off: &[(i64, i64)]) -> Vec<i64> {
    // 1970-01-01 was a Thursday, three days after a Monday.
    (from..=to)
        .filter(|day| (day + 3).rem_euclid(7) < 5 && !off_on(off, *day))
        .collect()
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use super::{day, working_days};
    use crate::sprint::tests::{relations, sprints, states};
    use crate::testing::{BASE, ado, batch, urls};

    fn item(
        id: i64,
        kind: &str,
        state: &str,
        who: Option<&str>,
        points: Option<f64>,
        left: Option<f64>,
    ) -> Value {
        let mut fields = json!({"System.WorkItemType": kind, "System.State": state,
            "System.IterationPath": "Fabrikam\\Sprint 13"});
        if let Some(who) = who {
            let first = who.split(' ').next().unwrap().to_lowercase();
            fields["System.AssignedTo"] =
                json!({"displayName": who, "uniqueName": format!("{first}@contoso.com")});
        }
        if let Some(points) = points {
            fields["Microsoft.VSTS.Scheduling.StoryPoints"] = json!(points);
        }
        if let Some(left) = left {
            fields["Microsoft.VSTS.Scheduling.RemainingWork"] = json!(left);
        }
        json!({"id": id, "rev": 1, "fields": fields})
    }

    #[test]
    fn working_days_skip_weekends_and_days_off() {
        let monday = day("2099-03-02T00:00:00Z").unwrap();
        let friday = day("2099-03-13T00:00:00Z").unwrap();
        assert_eq!(working_days(monday, friday, &[]).len(), 10);
        let wednesday = monday + 2;
        assert_eq!(
            working_days(monday, friday, &[(wednesday, wednesday + 1)]).len(),
            8
        );
        assert!(
            working_days(friday, monday, &[]).is_empty(),
            "a finished sprint has none"
        );
    }

    #[test]
    fn a_sprint_adds_up_its_items_and_each_persons_load_and_names_the_most_overloaded() {
        let mut elsewhere = item(9, "Task", "Active", Some("Sam Lee"), None, Some(50.0));
        elsewhere["fields"]["System.IterationPath"] = json!("Fabrikam\\Sprint 12");
        let (outcome, transport) = ado(
            &["ado", "sprint", "get", "@next"],
            vec![
                sprints(),
                relations(&[(1, None), (2, Some(1)), (3, Some(1)), (4, None), (9, None)]),
                batch(vec![
                    item(1, "User Story", "Active", Some("Jane Doe"), Some(5.0), None),
                    item(2, "Task", "Active", Some("Sam Lee"), None, Some(30.0)),
                    item(3, "Task", "Closed", Some("Jane Doe"), None, Some(4.0)),
                    item(4, "User Story", "Closed", None, Some(3.0), None),
                    elsewhere,
                ]),
                states(),
                states(),
                states(),
                states(),
                Answer::json(
                    &json!({"totalCapacityPerDay": 9, "totalDaysOff": 1, "teamMembers": [
                        {"teamMember": {"displayName": "Sam Lee", "uniqueName": "sam@contoso.com"},
                            "activities": [{"name": "Development", "capacityPerDay": 2},
                                {"name": "Testing", "capacityPerDay": 1}],
                            "daysOff": [{"start": "2099-03-04T00:00:00Z", "end": "2099-03-04T00:00:00Z"}]},
                        {"teamMember": {"displayName": "Jane Doe", "uniqueName": "jane@contoso.com"},
                            "activities": [{"name": "Development", "capacityPerDay": 6}], "daysOff": []}
                    ]}),
                ),
                Answer::json(
                    &json!({"daysOff": [{"start": "2099-03-13T00:00:00Z", "end": "2099-03-13T00:00:00Z"}]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({
                "id": "Fabrikam\\Sprint 13", "name": "Sprint 13", "path": "Fabrikam\\Sprint 13",
                "start": "2099-03-02T00:00:00Z", "finish": "2099-03-13T00:00:00Z", "timeframe": "future",
                "working_days_left": 9,
                "totals": {"items": 4, "by_state": {"Active": 2, "Closed": 2},
                    "points": 8.0, "points_done": 3.0, "remaining_work": 30.0},
                "people": [
                    {"name": "Jane Doe", "items": 2, "points": 5.0, "remaining_work": 0.0,
                        "capacity_per_day": 6.0, "days_off": 0, "capacity_left": 54.0},
                    {"name": "Sam Lee", "items": 1, "points": 0.0, "remaining_work": 30.0,
                        "capacity_per_day": 3.0, "days_off": 1, "capacity_left": 24.0}
                ],
                "team_days_off": [{"start": "2099-03-13T00:00:00Z", "end": "2099-03-13T00:00:00Z"}]
            })
        );
        assert_eq!(
            outcome.stderr,
            "[next: agent-cli ado workitem list --iteration 'Fabrikam\\Sprint 13' --assignee sam@contoso.com (Sam Lee has 30h of work left and 24h of capacity)]\n"
        );
        let sent = urls(&transport);
        let team = format!("{BASE}/Fabrikam/Web%20Team/_apis/work/teamsettings/iterations");
        assert_eq!(sent[1], format!("{team}/i-13/workitems?api-version=7.1"));
        assert_eq!(
            transport.sent()[2].body.as_ref().unwrap()["ids"],
            json!([1, 2, 3, 4, 9])
        );
        assert_eq!(
            sent[3],
            format!("{BASE}/Fabrikam/_apis/wit/workitemtypes/User%20Story/states?api-version=7.1")
        );
        assert_eq!(sent[7], format!("{team}/i-13/capacities?api-version=7.1"));
        assert_eq!(sent[8], format!("{team}/i-13/teamdaysoff?api-version=7.1"));
    }

    #[test]
    fn a_past_sprint_has_no_days_left_and_names_nobody() {
        let (outcome, _) = ado(
            &["ado", "sprint", "get", "Sprint 11", "--team", "Web Team"],
            vec![
                sprints(),
                relations(&[]),
                Answer::json(&json!({"teamMembers": [
                    {"teamMember": {"displayName": "Sam Lee", "uniqueName": "sam@contoso.com"},
                        "activities": [{"capacityPerDay": 6}], "daysOff": []}
                ]})),
                Answer::json(&json!({"daysOff": []})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sprint = outcome.json();
        assert_eq!(sprint["working_days_left"], 0);
        assert_eq!(sprint["totals"]["items"], 0);
        assert_eq!(sprint["people"][0]["capacity_left"], 0.0);
        assert_eq!(outcome.stderr, "");
    }
}
