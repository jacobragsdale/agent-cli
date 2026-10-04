use agent_cli_core::{Ctx, Failure, When, command};
use anyhow::Result;
use serde_json::json;

use crate::client::{Ado, list, text};
use crate::work_items::{WorkItemRow, rows};

/// The most ids one WIQL answer carries; more is a query to narrow.
const WIQL_TOP: usize = 20_000;

/// A WIQL string literal.
fn quoted(raw: &str) -> String {
    format!("'{}'", raw.trim().replace('\'', "''"))
}

fn one_of(field: &str, values: &[String]) -> String {
    match values {
        [one] => format!("[{field}] = {}", quoted(one)),
        many => format!(
            "[{field}] IN ({})",
            many.iter()
                .map(|value| quoted(value))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[derive(clap::Args)]
pub struct ListArgs {
    /// Name, email or @me
    #[arg(long)]
    assignee: Option<String>,
    /// Active, "In Progress" … (repeatable)
    #[arg(long, value_delimiter = ',')]
    state: Vec<String>,
    /// Bug, "User Story", Task … (repeatable)
    #[arg(long = "type", value_delimiter = ',')]
    work_item_type: Vec<String>,
    /// Iteration path or sprint name, or @current, @next or @previous for the team's sprint
    #[arg(long)]
    iteration: Option<String>,
    /// Area path (children included)
    #[arg(long)]
    area: Option<String>,
    /// A tag it carries (repeatable)
    #[arg(long)]
    tag: Vec<String>,
    /// 1 (highest) to 4 (repeatable)
    #[arg(long, value_delimiter = ',')]
    priority: Vec<i64>,
    /// Words in the title or description
    #[arg(long)]
    text: Option<String>,
    /// Changed (or --date created) after this
    #[arg(long)]
    since: Option<When>,
    /// Changed (or --date created) before this
    #[arg(long)]
    until: Option<When>,
    /// Which date --since and --until compare, and the newest first
    #[arg(long, value_enum, default_value = "changed")]
    date: DateField,
    /// Children of this work item
    #[arg(long)]
    parent: Option<i64>,
    /// Raw WIQL WHERE clause, ANDed with the rest
    #[arg(long)]
    wiql: Option<String>,
    /// Only work items that @mention you (the last 30 days)
    #[arg(long)]
    mentioned: bool,
    /// Only work items you follow
    #[arg(long)]
    following: bool,
    /// The team whose sprint @current, @next and @previous mean (default: [ado] team)
    #[arg(long)]
    team: Option<String>,
    /// Only this project (default: every project in [ado] project)
    #[arg(long)]
    project: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum DateField {
    Changed,
    Created,
}

/// The WHERE clause the flags spell, newest change (or creation) first. `iteration` is the
/// iteration condition already worked out, since `@current` may need a read.
fn wiql(ado: &Ado, args: &ListArgs, iteration: Option<String>, assignee: Option<String>) -> String {
    let mut conditions = vec![ado.projects_condition(args.project.is_some())];
    conditions.extend(assignee);
    if !args.state.is_empty() {
        conditions.push(one_of("System.State", &args.state));
    }
    if !args.work_item_type.is_empty() {
        conditions.push(one_of("System.WorkItemType", &args.work_item_type));
    }
    conditions.extend(iteration);
    if let Some(area) = &args.area {
        conditions.push(format!("[System.AreaPath] UNDER {}", quoted(area)));
    }
    for tag in &args.tag {
        conditions.push(format!("[System.Tags] CONTAINS {}", quoted(tag)));
    }
    if !args.priority.is_empty() {
        let priorities: Vec<String> = args.priority.iter().map(i64::to_string).collect();
        conditions.push(format!(
            "[Microsoft.VSTS.Common.Priority] IN ({})",
            priorities.join(", ")
        ));
    }
    if let Some(words) = &args.text {
        conditions.push(format!(
            "([System.Title] CONTAINS {0} OR [System.Description] CONTAINS WORDS {0})",
            quoted(words)
        ));
    }
    let date = match args.date {
        DateField::Changed => "System.ChangedDate",
        DateField::Created => "System.CreatedDate",
    };
    // Compared to the second, with timePrecision on the request.
    if let Some(since) = args.since {
        conditions.push(format!("[{date}] >= '{}'", since.utc()));
    }
    if let Some(until) = args.until {
        conditions.push(format!("[{date}] <= '{}'", until.utc()));
    }
    if let Some(parent) = args.parent {
        conditions.push(format!("[System.Parent] = {parent}"));
    }
    if args.mentioned {
        conditions.push("[System.Id] IN (@RecentMentions)".to_owned());
    }
    if args.following {
        conditions.push("[System.Id] IN (@Follows)".to_owned());
    }
    if let Some(raw) = &args.wiql {
        conditions.push(format!("({raw})"));
    }
    format!(
        "SELECT [System.Id] FROM WorkItems WHERE {} ORDER BY [{date}] DESC",
        conditions.join(" AND ")
    )
}

/// `--iteration` as a WIQL condition, and the team whose URL the query must
/// go to. `@current` is WIQL's own `@CurrentIteration` when one team is
/// named or configured (the macro reads the team from the URL); with several
/// (each project's, when the list spans them), each team's current sprint is
/// read from its settings. `@next` and `@previous` have no macro, so the
/// team's sprints say which path they are.
fn iteration_condition(
    ctx: &Ctx,
    ado: &Ado,
    team: Option<&str>,
    iteration: Option<&str>,
    every_project: bool,
) -> Result<(Option<String>, Option<String>)> {
    let Some(iteration) = iteration.map(str::trim) else {
        return Ok((None, None));
    };
    if !iteration.eq_ignore_ascii_case("@current") {
        let path = crate::iteration::path(ctx, ado, team, iteration)?;
        return Ok((
            Some(format!("[System.IterationPath] UNDER {}", quoted(&path))),
            None,
        ));
    }
    let teams: Vec<(String, String)> = match team {
        Some(team) => vec![(ado.project.clone(), team.to_owned())],
        None if every_project => ado.all_teams.clone(),
        None => (ado.teams.iter())
            .map(|team| (ado.project.clone(), team.clone()))
            .collect(),
    };
    match teams.as_slice() {
        [] if !ado.all_teams.is_empty() => Err(crate::iteration::no_team(ado)),
        [] => Err(Failure::setup(
            "--iteration @current means your team's sprint, and [ado] team is not set",
        )
        .hint("agent-cli ado team list --fields name, then set team = \"NAME\" under [ado] (or AGENT_CLI_ADO_TEAM)")
        .into()),
        [(project, team)] if *project == ado.project => Ok((
            Some("[System.IterationPath] = @CurrentIteration".to_owned()),
            Some(team.clone()),
        )),
        teams => {
            let mut sprints: Vec<String> = Vec::new();
            for (project, team) in teams {
                let url = ado.clone().in_project(project).team(
                    team,
                    "work/teamsettings/iterations",
                    "$timeframe=current",
                );
                let answer = ado.get(ctx, &url)?;
                if let Some(path) = text(&answer["value"][0]["path"])
                    && !sprints.contains(&path)
                {
                    sprints.push(path);
                }
            }
            if sprints.is_empty() {
                let names: Vec<&str> = teams.iter().map(|(_, team)| team.as_str()).collect();
                return Err(Failure::not_found(format!(
                    "none of the teams {} is in a sprint today",
                    names.join(", ")
                ))
                .hint("name the sprint: --iteration 'Project\\Sprint 12'")
                .into());
            }
            Ok((Some(one_of("System.IterationPath", &sprints)), None))
        }
    }
}

fn workitem_list(ctx: &Ctx, args: ListArgs) -> Result<Vec<WorkItemRow>> {
    if args
        .text
        .as_deref()
        .is_some_and(|text| text.trim().is_empty())
    {
        return Err(Failure::usage("--text needs a word to look for")
            .hint("agent-cli ado workitem list --text login --fields id,title")
            .into());
    }
    let ado = Ado::load_in(ctx, args.project.as_deref())?;
    let (iteration, team) = iteration_condition(
        ctx,
        &ado,
        args.team.as_deref(),
        args.iteration.as_deref(),
        args.project.is_none(),
    )?;
    // A name is resolved to one person, as --author is on pull requests,
    // so a part of one is not silently nobody; "" is the unassigned.
    let assignee = match args.assignee.as_deref().map(str::trim) {
        None => None,
        Some(who) if who.eq_ignore_ascii_case("@me") => {
            Some("[System.AssignedTo] = @Me".to_owned())
        }
        Some("") => Some("[System.AssignedTo] = ''".to_owned()),
        Some(who) => {
            let person = ado.person(ctx, who)?;
            Some(format!(
                "[System.AssignedTo] = {}",
                quoted(&person.email.unwrap_or(person.name))
            ))
        }
    };
    let query = wiql(&ado, &args, iteration, assignee);
    let mut top = format!("$top={WIQL_TOP}");
    if args.since.is_some() || args.until.is_some() {
        top.push_str("&timePrecision=true");
    }
    let url = match team {
        Some(team) => ado.team(&team, "wit/wiql", &top),
        None => ado.work("wit/wiql", &top),
    };
    let found = ado
        .query(ctx, &url, json!({ "query": query }))
        .map_err(|error| match error.downcast::<Failure>() {
            // TF51011: an iteration path the project does not have.
            Ok(failure) if failure.message.contains("TF51011") => failure
                .hint("agent-cli ado sprint list --fields id,timeframe")
                .into(),
            Ok(failure) => failure.into(),
            Err(error) => error,
        })?;
    let ids: Vec<i64> = list(&found["workItems"])
        .iter()
        .filter_map(|item| item["id"].as_i64())
        .collect();
    // A state no type has matches nothing, which would read as no work.
    if ids.is_empty() && !args.state.is_empty() {
        let projects = match args.project {
            Some(_) => std::slice::from_ref(&ado.project),
            None => ado.projects.as_slice(),
        };
        known_states(ctx, &ado, projects, &args.work_item_type, &args.state)?;
    }
    if ids.len() > args.limit {
        let more = if ids.len() >= WIQL_TOP { "+" } else { "" };
        ctx.note(format!(
            "[{} of {}{more}; --limit N, or narrow the filters]",
            args.limit,
            ids.len()
        ));
    }
    rows(ctx, &ado, &ids[..ids.len().min(args.limit)])
}

/// Exit 2 for a state none of `types` (else of the types) has in any of
/// `projects`: their processes may differ.
fn known_states(
    ctx: &Ctx,
    ado: &Ado,
    projects: &[String],
    types: &[String],
    states: &[String],
) -> Result<()> {
    let mut known: Vec<String> = Vec::new();
    let wanted = |kind: &&serde_json::Value| {
        types.is_empty()
            || types
                .iter()
                .any(|t| text(&kind["name"]).is_some_and(|n| n.eq_ignore_ascii_case(t.trim())))
    };
    for project in projects {
        let url = ado.api(Some(project), "wit/workitemtypes", "", crate::client::API);
        let answer = ado.get(ctx, &url)?;
        for kind in list(&answer["value"]).iter().filter(wanted) {
            for name in list(&kind["states"])
                .iter()
                .filter_map(|state| text(&state["name"]))
            {
                if !known.contains(&name) {
                    known.push(name);
                }
            }
        }
    }
    match states.iter().find(|state| {
        !known
            .iter()
            .any(|name| name.eq_ignore_ascii_case(state.trim()))
    }) {
        Some(state) => Err(Failure::usage(format!(
            "no work item {} the state {state:?}; the states are: {}",
            if types.is_empty() {
                "type has".to_owned()
            } else {
                format!("of type {} has", types.join(", "))
            },
            known.join(", ")
        ))
        .hint("agent-cli ado workitem-type list --fields name,states")
        .into()),
        None => Ok(()),
    }
}

command! {
    pub WORKITEM_LIST = ["ado", "workitem", "list"], Read,
    "List work items matching filters (live WIQL)",
    keywords: ["query", "find", "search", "assigned", "my", "mine", "sprint", "active", "open", "resolved", "wiql", "high", "urgent", "filed", "opened", "created", "mentions", "follow", "followed", "watching"],
    example: "ado workitem list --assignee @me --state Active --fields id,title,state",
    run: workitem_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, ado_with, batch, item, urls, wiql};

    fn query_of(transport: &agent_cli_core::testing::FakeTransport, at: usize) -> String {
        transport.sent()[at].body.as_ref().unwrap()["query"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn list_builds_wiql_from_the_flags_and_keeps_its_order_over_the_batch() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "workitem",
                "list",
                "--assignee",
                "@me",
                "--state",
                "Active,New",
                "--type",
                "Bug",
                "--area",
                "Fabrikam\\Web",
                "--tag",
                "ui",
                "--tag",
                "p1",
                "--text",
                "log in",
                "--since",
                "2026-09-22",
                "--parent",
                "7",
                "--wiql",
                "[System.Reason] <> 'Obsolete'",
                "--limit",
                "2",
            ],
            vec![
                wiql(&[30, 10, 20]),
                batch(vec![item(10, 3, "Older"), item(30, 9, "Newest")]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["id"], 30);
        assert_eq!(rows[1]["id"], 10);
        assert_eq!(rows[0]["type"], "Bug");
        assert_eq!(rows[0]["assignee"], "Jane Doe");
        assert_eq!(rows[0]["tags"], json!(["ui", "p1"]));
        assert_eq!(rows[0]["rev"], 9);
        assert_eq!(
            outcome.stderr,
            "[2 of 3; --limit N, or narrow the filters]\n"
        );
        let sent = transport.sent();
        assert!(
            sent[0].method.is_read() && sent[1].method.is_read(),
            "WIQL and the batch are reads"
        );
        assert_eq!(
            sent[0].url,
            format!("{BASE}/Fabrikam/_apis/wit/wiql?$top=20000&timePrecision=true&api-version=7.1")
        );
        assert_eq!(
            query_of(&transport, 0),
            "SELECT [System.Id] FROM WorkItems WHERE [System.TeamProject] = @project \
             AND [System.AssignedTo] = @Me AND [System.State] IN ('Active', 'New') \
             AND [System.WorkItemType] = 'Bug' AND [System.AreaPath] UNDER 'Fabrikam\\Web' \
             AND [System.Tags] CONTAINS 'ui' AND [System.Tags] CONTAINS 'p1' \
             AND ([System.Title] CONTAINS 'log in' OR [System.Description] CONTAINS WORDS 'log in') \
             AND [System.ChangedDate] >= '2026-09-22T00:00:00Z' AND [System.Parent] = 7 \
             AND ([System.Reason] <> 'Obsolete') ORDER BY [System.ChangedDate] DESC"
        );
        assert_eq!(
            sent[1].url,
            format!("{BASE}/Fabrikam/_apis/wit/workitemsbatch?api-version=7.1")
        );
        assert_eq!(sent[1].body.as_ref().unwrap()["ids"], json!([30, 10]));
        assert_eq!(
            sent[1].authorization.as_deref(),
            Some("Bearer token@499b84ac-1321-427f-aa17-267ca6975798"),
            "an az token for Azure DevOps's own resource"
        );
    }

    #[test]
    fn current_iteration_is_wiqls_own_macro_through_the_one_teams_url() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "workitem",
                "list",
                "--iteration",
                "@current",
                "--assignee",
                "Sam O'Neil",
            ],
            vec![
                crate::testing::person("p-9", "Sam O'Neil", "sam.oneil@contoso.com"),
                wiql(&[]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.stdout.trim(), "[]");
        assert_eq!(
            urls(&transport)[1],
            format!("{BASE}/Fabrikam/Web%20Team/_apis/wit/wiql?$top=20000&api-version=7.1"),
            "the name resolved to a person, then the query; no batch for no ids"
        );
        let query = query_of(&transport, 1);
        assert!(
            query.contains("[System.AssignedTo] = 'sam.oneil@contoso.com'"),
            "{query}"
        );
        assert!(
            query.contains("[System.IterationPath] = @CurrentIteration"),
            "{query}"
        );
    }

    #[test]
    fn next_and_previous_iteration_are_the_teams_sprint_paths() {
        let sprints = Answer::json(&json!({"count": 2, "value": [
            {"id": "i-12", "name": "Sprint 12", "path": "Fabrikam\\Sprint 12",
                "attributes": {"timeFrame": "current"}},
            {"id": "i-13", "name": "Sprint 13", "path": "Fabrikam\\Sprint 13",
                "attributes": {"timeFrame": "future"}}
        ]}));
        let (outcome, named) = ado(
            &["ado", "workitem", "list", "--iteration", "sprint 12"],
            vec![sprints.clone(), wiql(&[])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let query = query_of(&named, 1);
        assert!(
            query.contains("[System.IterationPath] UNDER 'Fabrikam\\Sprint 12'"),
            "{query}"
        );
        let (outcome, transport) = ado(
            &["ado", "workitem", "list", "--iteration", "@next"],
            vec![sprints, wiql(&[])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport),
            [
                format!(
                    "{BASE}/Fabrikam/Web%20Team/_apis/work/teamsettings/iterations?api-version=7.1"
                ),
                format!("{BASE}/Fabrikam/_apis/wit/wiql?$top=20000&api-version=7.1"),
            ]
        );
        let query = query_of(&transport, 1);
        assert!(
            query.contains("[System.IterationPath] UNDER 'Fabrikam\\Sprint 13'"),
            "{query}"
        );
    }

    #[test]
    fn current_iteration_reads_each_teams_sprint_when_several_are_configured() {
        let config =
            "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\nteam = [\"Web\", \"Data\"]\n";
        let sprint = |path: &str| Answer::json(&json!({"count": 1, "value": [{"path": path}]}));
        let (outcome, transport) = ado_with(
            config,
            &["ado", "workitem", "list", "--iteration", "@current"],
            vec![
                sprint("Fabrikam\\Sprint 12"),
                sprint("Fabrikam\\Data 4"),
                wiql(&[]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = urls(&transport);
        assert_eq!(
            sent[0],
            format!(
                "{BASE}/Fabrikam/Web/_apis/work/teamsettings/iterations?$timeframe=current&api-version=7.1"
            )
        );
        assert!(sent[2].starts_with(&format!("{BASE}/Fabrikam/_apis/wit/wiql")));
        let query = query_of(&transport, 2);
        assert!(
            query.contains("[System.IterationPath] IN ('Fabrikam\\Sprint 12', 'Fabrikam\\Data 4')"),
            "{query}"
        );
    }

    const PROJECTS: &str = "[ado]\norg = \"contoso\"\nproject = [\"Fabrikam\", \"Contoso Mobile\"]\nteam = [\"Web Team\", \"Contoso Mobile/Apps\"]\n";

    #[test]
    fn several_projects_are_one_query_and_project_narrows_it_to_one() {
        let (outcome, transport) = ado_with(
            PROJECTS,
            &["ado", "workitem", "list", "--iteration", "@current"],
            vec![
                Answer::json(&json!({"count": 1, "value": [{"path": "Fabrikam\\Sprint 12"}]})),
                Answer::json(&json!({"count": 1, "value": [{"path": "Contoso Mobile\\M 3"}]})),
                wiql(&[]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = urls(&transport);
        assert_eq!(
            sent[1],
            format!(
                "{BASE}/Contoso%20Mobile/Apps/_apis/work/teamsettings/iterations?$timeframe=current&api-version=7.1"
            ),
            "each project's team, under its project"
        );
        assert!(sent[2].starts_with(&format!("{BASE}/Fabrikam/_apis/wit/wiql")));
        assert!(
            query_of(&transport, 2).starts_with(
                "SELECT [System.Id] FROM WorkItems WHERE [System.TeamProject] IN ('Fabrikam', 'Contoso Mobile') \
                 AND [System.IterationPath] IN ('Fabrikam\\Sprint 12', 'Contoso Mobile\\M 3')"
            ),
            "{}",
            query_of(&transport, 2)
        );

        let (outcome, transport) = ado_with(
            PROJECTS,
            &[
                "ado",
                "workitem",
                "list",
                "--project",
                "contoso mobile",
                "--iteration",
                "@current",
            ],
            vec![wiql(&[])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            urls(&transport)[0]
                .starts_with(&format!("{BASE}/Contoso%20Mobile/Apps/_apis/wit/wiql")),
            "its one team's @CurrentIteration"
        );
        assert!(
            query_of(&transport, 0).starts_with(
                "SELECT [System.Id] FROM WorkItems WHERE [System.TeamProject] = @project \
                 AND [System.IterationPath] = @CurrentIteration"
            ),
            "{}",
            query_of(&transport, 0)
        );
    }

    #[test]
    fn a_state_only_another_projects_process_has_is_no_error() {
        let types = |states: &[&str]| {
            let states: Vec<_> = states.iter().map(|name| json!({"name": name})).collect();
            Answer::json(&json!({"count": 1, "value": [{"name": "Task", "states": states}]}))
        };
        let (outcome, transport) = ado_with(
            PROJECTS,
            &["ado", "workitem", "list", "--state", "Closed"],
            vec![
                wiql(&[]),
                types(&["To Do", "Done"]),
                types(&["New", "Closed"]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport)[2],
            format!("{BASE}/Contoso%20Mobile/_apis/wit/workitemtypes?api-version=7.1")
        );
    }

    #[test]
    fn current_iteration_without_a_team_is_needs_setup_and_a_bad_date_is_usage() {
        let config = "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\n";
        let (outcome, transport) = ado_with(
            config,
            &["ado", "workitem", "list", "--iteration", "@current"],
            vec![],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains("hint: agent-cli ado team list"),
            "{}",
            outcome.stderr
        );
        assert!(transport.sent().is_empty());

        let (outcome, _) = ado(&["ado", "workitem", "list", "--since", "last week"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("expected a time: 15m"),
            "{}",
            outcome.stderr
        );
        let (outcome, transport) = ado(
            &[
                "ado",
                "workitem",
                "list",
                "--since",
                "2026-09-01T12:00:00+02:00",
                "--until",
                "2026-09-02",
            ],
            vec![wiql(&[])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let query = query_of(&transport, 0);
        assert!(
            query.contains(
                "[System.ChangedDate] >= '2026-09-01T10:00:00Z' AND [System.ChangedDate] <= '2026-09-02T00:00:00Z'"
            ),
            "{query}"
        );
    }

    #[test]
    fn priorities_are_one_condition_and_date_created_moves_the_window_and_the_order() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "workitem",
                "list",
                "--type",
                "Bug",
                "--priority",
                "1,2",
                "--date",
                "created",
                "--since",
                "7d",
            ],
            vec![wiql(&[])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let query = query_of(&transport, 0);
        assert!(
            query.contains(
                "AND [System.WorkItemType] = 'Bug' AND [Microsoft.VSTS.Common.Priority] IN (1, 2) \
                 AND [System.CreatedDate] >= '"
            ),
            "{query}"
        );
        assert!(
            query.ends_with("ORDER BY [System.CreatedDate] DESC"),
            "{query}"
        );
        assert!(!query.contains("ChangedDate"), "{query}");

        let (outcome, _) = ado(&["ado", "workitem", "list", "--priority", "high"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn mentioned_and_following_are_wiql_macros_and_team_picks_whose_current_sprint() {
        let config =
            "[ado]\norg = \"contoso\"\nproject = \"Fabrikam\"\nteam = [\"Web\", \"Data\"]\n";
        let (outcome, transport) = ado_with(
            config,
            &[
                "ado",
                "workitem",
                "list",
                "--mentioned",
                "--following",
                "--iteration",
                "@current",
                "--team",
                "Data",
            ],
            vec![wiql(&[])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            urls(&transport),
            [format!(
                "{BASE}/Fabrikam/Data/_apis/wit/wiql?$top=20000&api-version=7.1"
            )],
            "the named team's macro, not each configured team's sprint"
        );
        let query = query_of(&transport, 0);
        assert!(
            query.contains(
                "[System.IterationPath] = @CurrentIteration AND [System.Id] IN (@RecentMentions) \
                 AND [System.Id] IN (@Follows)"
            ),
            "{query}"
        );
    }

    #[test]
    fn an_empty_answer_for_a_state_no_type_has_is_exit_2_naming_the_states() {
        let types = || {
            Answer::json(&json!({"value": [
                {"name": "Task", "states": [{"name": "To Do"}, {"name": "Done"}]},
                {"name": "Test Case", "states": [{"name": "Design"}, {"name": "Closed"}]}
            ]}))
        };
        let (outcome, _) = ado(
            &["ado", "workitem", "list", "--state", "all"],
            vec![wiql(&[]), types()],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("no work item type has the state \"all\"; the states are: To Do, Done, Design, Closed"),
            "{}",
            outcome.stderr
        );
        let argv = [
            "ado", "workitem", "list", "--type", "task", "--state", "Closed",
        ];
        let (outcome, _) = ado(&argv, vec![wiql(&[]), types()]);
        assert!(
            outcome
                .stderr
                .contains("of type task has the state \"Closed\""),
            "{}",
            outcome.stderr
        );
        let (outcome, _) = ado(
            &["ado", "workitem", "list", "--state", "done"],
            vec![wiql(&[]), types()],
        );
        assert_eq!(
            (outcome.code, outcome.stdout.trim()),
            (0, "[]"),
            "{outcome:?}"
        );
    }
}
