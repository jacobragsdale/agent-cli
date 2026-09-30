//! Work items: the WIQL-backed list, one work item with its links and latest
//! comments, and the writes (create, update, comment, link to a branch). Plus
//! `team list`, which is where `[ado] team` comes from.

use std::collections::HashMap;
use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Exit, Failure, Method, When, command};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{
    Ado, COMMENTS_API, Kind, RepoRef, list, query_value, segment, short_branch, stamp, text,
};
use crate::markdown::{CommentBody, html_to_markdown, markdown_to_html};

/// The fields a list row carries. The batch endpoint returns only these, so
/// fifty rows cost one small request.
const ROW_FIELDS: [&str; 10] = [
    "System.WorkItemType",
    "System.Title",
    "System.State",
    "System.AssignedTo",
    "System.IterationPath",
    "System.AreaPath",
    "Microsoft.VSTS.Common.Priority",
    "System.Tags",
    "System.ChangedDate",
    "System.Id",
];
/// The most ids one WIQL answer carries; more is a query to narrow.
const WIQL_TOP: usize = 20_000;
/// The largest id batch the work item endpoints accept.
const BATCH: usize = 200;
/// The link type a parent is held under, on the child's side.
const PARENT: &str = "System.LinkTypes.Hierarchy-Reverse";
const CHILD: &str = "System.LinkTypes.Hierarchy-Forward";
const RELATED: &str = "System.LinkTypes.Related";

/// One work item as a list shows it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkItemRow {
    id: i64,
    #[serde(rename = "type")]
    kind: Option<String>,
    title: Option<String>,
    state: Option<String>,
    assignee: Option<String>,
    iteration: Option<String>,
    area: Option<String>,
    priority: Option<i64>,
    tags: Vec<String>,
    /// When it last changed, RFC 3339.
    changed: Option<String>,
    /// Its revision, for `workitem update --if-rev`.
    rev: Option<i64>,
}

/// A work item named by something else (a run, a pull request): enough to
/// say what it is, with the id `ado workitem get` takes.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkItemRef {
    id: i64,
    #[serde(rename = "type")]
    kind: Option<String>,
    title: Option<String>,
    state: Option<String>,
}

impl From<&WorkItemRow> for WorkItemRef {
    fn from(row: &WorkItemRow) -> Self {
        Self {
            id: row.id,
            kind: row.kind.clone(),
            title: row.title.clone(),
            state: row.state.clone(),
        }
    }
}

fn row(item: &Value) -> WorkItemRow {
    let fields = &item["fields"];
    let field = |name: &str| text(&fields[name]);
    WorkItemRow {
        id: item["id"]
            .as_i64()
            .or_else(|| fields["System.Id"].as_i64())
            .unwrap_or_default(),
        kind: field("System.WorkItemType"),
        title: field("System.Title"),
        state: field("System.State"),
        assignee: person(&fields["System.AssignedTo"]),
        iteration: field("System.IterationPath"),
        area: field("System.AreaPath"),
        priority: fields["Microsoft.VSTS.Common.Priority"].as_i64(),
        tags: field("System.Tags")
            .map(|tags| {
                tags.split(';')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        changed: stamp(&fields["System.ChangedDate"]),
        rev: item["rev"].as_i64(),
    }
}

/// An identity field's display name; older answers hold a plain string.
fn person(value: &Value) -> Option<String> {
    text(&value["displayName"])
        .or_else(|| text(&value["uniqueName"]))
        .or_else(|| text(value))
}

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

// ---------- ado workitem list ----------

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
    /// Iteration path, or @current for the team's sprint
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
fn wiql(args: &ListArgs, iteration: Option<String>) -> String {
    let mut conditions = vec!["[System.TeamProject] = @project".to_owned()];
    if let Some(who) = &args.assignee {
        conditions.push(if who.trim().eq_ignore_ascii_case("@me") {
            "[System.AssignedTo] = @Me".to_owned()
        } else {
            format!("[System.AssignedTo] = {}", quoted(who))
        });
    }
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
/// configured (the macro reads the team from the URL); with several, each
/// team's current sprint is read from its settings.
fn iteration_condition(
    ctx: &Ctx,
    ado: &Ado,
    iteration: Option<&str>,
) -> Result<(Option<String>, Option<String>)> {
    let Some(iteration) = iteration.map(str::trim) else {
        return Ok((None, None));
    };
    if !iteration.eq_ignore_ascii_case("@current") {
        return Ok((
            Some(format!(
                "[System.IterationPath] UNDER {}",
                quoted(iteration)
            )),
            None,
        ));
    }
    match ado.teams.as_slice() {
        [] => Err(Failure::setup(
            "--iteration @current means your team's sprint, and [ado] team is not set",
        )
        .hint("agent-cli ado team list --fields name, then set team = \"NAME\" under [ado] (or AGENT_CLI_ADO_TEAM)")
        .into()),
        [team] => Ok((
            Some("[System.IterationPath] = @CurrentIteration".to_owned()),
            Some(team.clone()),
        )),
        teams => {
            let mut sprints: Vec<String> = Vec::new();
            for team in teams {
                let url = ado.team(team, "work/teamsettings/iterations", "$timeframe=current");
                let answer = ado.get(ctx, &url)?;
                if let Some(path) = text(&answer["value"][0]["path"])
                    && !sprints.contains(&path)
                {
                    sprints.push(path);
                }
            }
            if sprints.is_empty() {
                return Err(Failure::not_found(format!(
                    "none of the teams {} is in a sprint today",
                    teams.join(", ")
                ))
                .hint("name the sprint: --iteration 'Project\\Sprint 12'")
                .into());
            }
            Ok((Some(one_of("System.IterationPath", &sprints)), None))
        }
    }
}

/// The rows for `ids`, in the order given: the batch endpoint does not
/// promise the WIQL's order.
pub(crate) fn rows(ctx: &Ctx, ado: &Ado, ids: &[i64]) -> Result<Vec<WorkItemRow>> {
    let mut rows = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(BATCH) {
        let answer = ado.query(
            ctx,
            &ado.work("wit/workitemsbatch", ""),
            json!({"ids": chunk, "fields": ROW_FIELDS, "errorPolicy": "omit"}),
        )?;
        rows.extend(
            list(&answer["value"])
                .iter()
                .filter(|item| !item.is_null())
                .map(row),
        );
    }
    let rank: HashMap<i64, usize> = ids.iter().enumerate().map(|(at, id)| (*id, at)).collect();
    rows.sort_by_key(|row| rank.get(&row.id).copied().unwrap_or(usize::MAX));
    Ok(rows)
}

fn workitem_list(ctx: &Ctx, args: ListArgs) -> Result<Vec<WorkItemRow>> {
    let ado = Ado::load(ctx)?;
    let (iteration, team) = iteration_condition(ctx, &ado, args.iteration.as_deref())?;
    let query = wiql(&args, iteration);
    let mut top = format!("$top={WIQL_TOP}");
    if args.since.is_some() || args.until.is_some() {
        top.push_str("&timePrecision=true");
    }
    let url = match team {
        Some(team) => ado.team(&team, "wit/wiql", &top),
        None => ado.work("wit/wiql", &top),
    };
    let found = ado.query(ctx, &url, json!({ "query": query }))?;
    let ids: Vec<i64> = list(&found["workItems"])
        .iter()
        .filter_map(|item| item["id"].as_i64())
        .collect();
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

command! {
    pub WORKITEM_LIST = ["ado", "workitem", "list"], Read,
    "List work items matching filters (live WIQL)",
    keywords: ["query", "find", "search", "assigned", "my", "mine", "sprint", "active", "open", "resolved", "wiql", "high", "urgent", "filed", "opened", "created"],
    example: "ado workitem list --assignee @me --state Active --fields id,title,state",
    run: workitem_list,
}

// ---------- ado workitem get ----------

#[derive(clap::Args)]
pub struct GetArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// How many of the latest comments to include
    #[arg(long, default_value_t = 5)]
    comments: usize,
}

/// One work item, its text as Markdown, its links and its latest comments.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkItem {
    #[serde(flatten)]
    row: WorkItemRow,
    parent: Option<i64>,
    children: Vec<i64>,
    related: Vec<i64>,
    pull_requests: Vec<PrLink>,
    branches: Vec<BranchLink>,
    /// Markdown.
    description: Option<String>,
    /// Markdown.
    acceptance_criteria: Option<String>,
    comment_count: Option<i64>,
    /// The latest first.
    comments: Vec<Comment>,
    url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrLink {
    repo: String,
    id: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BranchLink {
    repo: String,
    name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Comment {
    id: i64,
    author: Option<String>,
    date: Option<String>,
    /// Markdown.
    text: String,
}

/// What a `vstfs:///` artifact link points at, when it is something this
/// command shows: `vstfs:///Git/PullRequestId/{project}%2F{repo}%2F{id}` or
/// `vstfs:///Git/Ref/{project}%2F{repo}%2FGB{branch}`.
#[derive(Debug, PartialEq)]
enum Artifact {
    PullRequest { repo_id: String, id: i64 },
    Branch { repo_id: String, name: String },
}

fn artifact(url: &str) -> Option<Artifact> {
    let rest = url.strip_prefix("vstfs:///Git/")?;
    let (kind, id) = rest.split_once('/')?;
    // The separators inside the id are percent-encoded, in either case.
    let parts: Vec<&str> = id
        .split('/')
        .flat_map(|part| part.split("%2F"))
        .flat_map(|part| part.split("%2f"))
        .collect();
    match (kind, parts.as_slice()) {
        ("PullRequestId", [_project, repo_id, id]) => Some(Artifact::PullRequest {
            repo_id: (*repo_id).to_owned(),
            id: id.parse().ok()?,
        }),
        // A branch name carries its own slashes, encoded like the separators.
        ("Ref", [_project, repo_id, rest @ ..]) => {
            let name = rest.join("/");
            let name = name.strip_prefix("GB")?;
            (!name.is_empty()).then(|| Artifact::Branch {
                repo_id: (*repo_id).to_owned(),
                name: name.to_owned(),
            })
        }
        _ => None,
    }
}

/// The work item's own links: `(rel, url)`.
fn relations(item: &Value) -> impl Iterator<Item = (&str, &str)> {
    list(&item["relations"])
        .iter()
        .filter_map(|relation| Some((relation["rel"].as_str()?, relation["url"].as_str()?)))
}

fn linked_id(url: &str) -> Option<i64> {
    url.rsplit('/').next()?.parse().ok()
}

fn workitem_get(ctx: &Ctx, args: GetArgs) -> Result<WorkItem> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let item = ado.get(
        ctx,
        &ado.api(
            None,
            &format!("wit/workitems/{id}"),
            "$expand=relations",
            crate::client::API,
        ),
    )?;
    let mut work = WorkItem {
        row: row(&item),
        parent: None,
        children: Vec::new(),
        related: Vec::new(),
        pull_requests: Vec::new(),
        branches: Vec::new(),
        description: text(&item["fields"]["System.Description"])
            .map(|html| html_to_markdown(&html)),
        acceptance_criteria: text(&item["fields"]["Microsoft.VSTS.Common.AcceptanceCriteria"])
            .map(|html| html_to_markdown(&html)),
        comment_count: None,
        comments: Vec::new(),
        url: ado.work_item_url(id),
    };
    let mut artifacts = Vec::new();
    for (rel, url) in relations(&item) {
        match rel {
            PARENT => work.parent = linked_id(url),
            CHILD => work.children.extend(linked_id(url)),
            RELATED => work.related.extend(linked_id(url)),
            "ArtifactLink" => artifacts.extend(artifact(url)),
            _ => {}
        }
    }
    if !artifacts.is_empty() {
        let repos = ado.repos(ctx, false)?;
        let name = |repo_id: &str| {
            repos
                .iter()
                .find(|repo| repo.id.eq_ignore_ascii_case(repo_id))
                .map_or_else(|| repo_id.to_owned(), |repo| repo.name.clone())
        };
        for artifact in artifacts {
            match artifact {
                Artifact::PullRequest { repo_id, id } => work.pull_requests.push(PrLink {
                    repo: name(&repo_id),
                    id,
                }),
                Artifact::Branch {
                    repo_id,
                    name: branch,
                } => work.branches.push(BranchLink {
                    repo: name(&repo_id),
                    name: branch,
                }),
            }
        }
    }
    if args.comments > 0 {
        let url = ado.api(
            Some(&ado.project),
            &format!("wit/workItems/{id}/comments"),
            &format!("$top={}&order=desc", args.comments),
            COMMENTS_API,
        );
        let page = ado.get(ctx, &url)?;
        work.comment_count = page["totalCount"].as_i64();
        work.comments = list(&page["comments"])
            .iter()
            .filter_map(|comment| {
                let text = html_to_markdown(comment["text"].as_str().unwrap_or_default());
                (!text.is_empty()).then(|| Comment {
                    id: comment["id"].as_i64().unwrap_or_default(),
                    author: person(&comment["createdBy"]),
                    date: stamp(&comment["createdDate"]),
                    text,
                })
            })
            .collect();
    }
    Ok(work)
}

command! {
    pub WORKITEM_GET = ["ado", "workitem", "get"], Read,
    "Show a work item: fields, description as Markdown, links, latest comments",
    keywords: ["ticket", "details", "description", "acceptance", "criteria", "parent", "children", "read"],
    example: "ado workitem get 42 --fields id,title,state,description",
    run: workitem_get,
}

// ---------- the fields create and update write ----------

#[derive(clap::Args)]
struct Fields {
    /// Active, Closed …
    #[arg(long)]
    state: Option<String>,
    /// Name, email or @me ("" unassigns)
    #[arg(long)]
    assignee: Option<String>,
    /// Full iteration path
    #[arg(long)]
    iteration: Option<String>,
    /// Full area path
    #[arg(long)]
    area: Option<String>,
    /// 1 (highest) to 4
    #[arg(long)]
    priority: Option<i64>,
    /// Comma-separated; replaces the tags it has
    #[arg(long)]
    tags: Option<String>,
    /// Markdown, stored as HTML; - reads stdin
    #[arg(long, allow_hyphen_values = true)]
    description: Option<String>,
    /// The description from a Markdown file
    #[arg(long)]
    description_file: Option<PathBuf>,
    /// Markdown, stored as HTML; - reads stdin
    #[arg(long, allow_hyphen_values = true)]
    acceptance_criteria: Option<String>,
    /// The acceptance criteria from a Markdown file
    #[arg(long)]
    acceptance_criteria_file: Option<PathBuf>,
}

/// One JSON Patch operation setting a field. Azure DevOps takes `add` for a
/// field that is already set as well as for one that is not.
fn set(field: &str, value: impl Into<Value>) -> Value {
    json!({"op": "add", "path": format!("/fields/{field}"), "value": value.into()})
}

/// The operations that write `fields`, in the order the flags are documented.
fn field_ops(ctx: &Ctx, ado: &Ado, title: Option<&str>, fields: &Fields) -> Result<Vec<Value>> {
    let mut ops = Vec::new();
    if let Some(title) = title {
        let title = title.trim();
        if title.is_empty() {
            return Err(Failure::usage("a work item cannot have an empty title").into());
        }
        ops.push(set("System.Title", title));
    }
    if let Some(state) = &fields.state {
        ops.push(set("System.State", state.trim()));
    }
    if let Some(who) = &fields.assignee {
        let who = who.trim();
        ops.push(if who.is_empty() {
            // Nobody is written by removing the field, not by an empty name.
            json!({"op": "remove", "path": "/fields/System.AssignedTo"})
        } else if who.eq_ignore_ascii_case("@me") {
            let me = ado.me(ctx)?;
            set("System.AssignedTo", me.account.unwrap_or(me.name))
        } else {
            set("System.AssignedTo", who)
        });
    }
    if let Some(iteration) = &fields.iteration {
        ops.push(set("System.IterationPath", iteration.trim()));
    }
    if let Some(area) = &fields.area {
        ops.push(set("System.AreaPath", area.trim()));
    }
    if let Some(priority) = fields.priority {
        ops.push(set("Microsoft.VSTS.Common.Priority", priority));
    }
    if let Some(tags) = &fields.tags {
        ops.push(set("System.Tags", tag_list(tags)));
    }
    if fields.description.as_deref() == Some("-")
        && fields.acceptance_criteria.as_deref() == Some("-")
    {
        return Err(Failure::usage(
            "--description and --acceptance-criteria cannot both read stdin",
        )
        .hint("pipe one, and pass the other as --acceptance-criteria-file PATH")
        .into());
    }
    for (name, field, typed, file) in [
        (
            "description",
            "System.Description",
            &fields.description,
            &fields.description_file,
        ),
        (
            "acceptance-criteria",
            "Microsoft.VSTS.Common.AcceptanceCriteria",
            &fields.acceptance_criteria,
            &fields.acceptance_criteria_file,
        ),
    ] {
        if let Some(markdown) = ctx.long_text(name, typed.as_deref(), file.as_deref(), None)? {
            ops.push(set(field, markdown_to_html(&markdown.text)));
        }
    }
    Ok(ops)
}

/// `a, b,A` as `System.Tags` holds it: `a; b`, each tag once whatever its case.
fn tag_list(raw: &str) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut tags: Vec<&str> = Vec::new();
    for tag in raw
        .split([',', ';'])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
    {
        let folded = tag.to_lowercase();
        if !seen.contains(&folded) {
            seen.push(folded);
            tags.push(tag);
        }
    }
    tags.join("; ")
}

// ---------- ado workitem create ----------

#[derive(clap::Args)]
pub struct CreateArgs {
    /// Bug, Task, "User Story" …
    #[arg(long = "type")]
    work_item_type: String,
    /// What it is called
    #[arg(long)]
    title: String,
    /// The work item it goes under
    #[arg(long)]
    parent: Option<i64>,
    #[command(flatten)]
    fields: Fields,
}

fn workitem_create(ctx: &Ctx, args: CreateArgs) -> Result<WorkItemRow> {
    let ado = Ado::load(ctx)?;
    let mut document = field_ops(ctx, &ado, Some(&args.title), &args.fields)?;
    if let Some(parent) = args.parent {
        // A parent is a link appended to the new work item, not a field.
        document.push(json!({
            "op": "add",
            "path": "/relations/-",
            "value": {
                "rel": PARENT,
                "url": format!("{}/_apis/wit/workItems/{parent}", ado.base()),
            },
        }));
    }
    // The type is a path segment with a literal `$` in front of it.
    let url = ado.work(
        &format!("wit/workitems/${}", segment(args.work_item_type.trim())),
        "",
    );
    let created = ado.patch_work_item(ctx, Method::Post, &url, document)?;
    Ok(row(&created))
}

command! {
    pub WORKITEM_CREATE = ["ado", "workitem", "create"], Write,
    "Create a work item (bug, task, story …), optionally under a parent",
    keywords: ["new", "file", "open", "add", "ticket", "bug", "story"],
    example: "ado workitem create --type Bug --title 'Login fails on Safari' --priority 2",
    run: workitem_create,
}

// ---------- ado workitem update ----------

#[derive(clap::Args)]
pub struct UpdateArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// A new title
    #[arg(long)]
    title: Option<String>,
    #[command(flatten)]
    fields: Fields,
    /// Refuse unless it is still at this rev (from workitem get)
    #[arg(long)]
    if_rev: Option<i64>,
}

fn workitem_update(ctx: &Ctx, args: UpdateArgs) -> Result<WorkItemRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let changes = field_ops(ctx, &ado, args.title.as_deref(), &args.fields)?;
    if changes.is_empty() {
        return Err(Failure::usage("nothing to change")
            .hint(format!(
                "pass at least one of --state --assignee --title --iteration --area --priority --tags --description --acceptance-criteria, e.g. agent-cli ado workitem update {id} --state Active"
            ))
            .into());
    }
    // The test leads, so a work item that moved on refuses the whole document.
    let mut document: Vec<Value> = args
        .if_rev
        .map(|rev| json!({"op": "test", "path": "/rev", "value": rev}))
        .into_iter()
        .collect();
    document.extend(changes);
    let url = ado.api(None, &format!("wit/workitems/{id}"), "", crate::client::API);
    let updated = ado
        .patch_work_item(ctx, Method::Patch, &url, document)
        .map_err(|error| moved_on(error, id))?;
    Ok(row(&updated))
}

/// A refused write that means the work item changed since it was read, as
/// exit 5 with the way back. Azure DevOps reports a failed `test /rev` as a
/// 4xx whose message talks about the revision or the test.
fn moved_on(error: anyhow::Error, id: i64) -> anyhow::Error {
    let said = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<Failure>())
        .filter(|failure| {
            let message = failure.message.to_ascii_lowercase();
            failure.exit == Exit::Conflict
                || (failure.exit == Exit::Usage
                    && ["/rev", "revision", "test operation", "changed by another"]
                        .iter()
                        .any(|needle| message.contains(needle)))
        })
        .map(|failure| failure.message.clone());
    match said {
        Some(message) => Failure::conflict(format!("work item {id} changed since it was read: {message}"))
            .hint(format!(
                "re-read it (agent-cli ado workitem get {id} --fields rev,state,assignee), then run it again with the new --if-rev"
            ))
            .into(),
        None => error,
    }
}

command! {
    pub WORKITEM_UPDATE = ["ado", "workitem", "update"], Write,
    "Change a work item's state, assignee, title, iteration, tags or description",
    keywords: ["edit", "move", "reassign", "close", "resolve", "reopen", "set", "rev", "markdown"],
    example: "ado workitem update 42 --state Active --assignee @me --if-rev 7",
    run: workitem_update,
}

// ---------- ado workitem comment ----------

#[derive(clap::Args)]
pub struct CommentArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// Markdown, or - to read stdin (posted as a code block, 64 KiB max)
    #[arg(allow_hyphen_values = true, required_unless_present = "text_file")]
    text: Option<String>,
    /// The comment from a Markdown file (64 KiB max)
    #[arg(long)]
    text_file: Option<PathBuf>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CommentPosted {
    work_item: i64,
    /// The comment's id.
    id: Option<i64>,
    date: Option<String>,
}

fn workitem_comment(ctx: &Ctx, args: CommentArgs) -> Result<CommentPosted> {
    let body = CommentBody::read(ctx, args.text.as_deref(), args.text_file.as_deref())?;
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let url = ado.api(
        Some(&ado.project),
        &format!("wit/workItems/{}/comments", id),
        "",
        COMMENTS_API,
    );
    let posted = ado.change(
        ctx,
        Effect::Write,
        Method::Post,
        &url,
        json!({"text": body.html()}),
    )?;
    Ok(CommentPosted {
        work_item: id,
        id: posted["id"].as_i64(),
        date: stamp(&posted["createdDate"]),
    })
}

command! {
    pub WORKITEM_COMMENT = ["ado", "workitem", "comment"], Write,
    "Add a comment to a work item (Markdown, - for stdin, or --text-file)",
    keywords: ["note", "discussion", "reply", "post", "ticket"],
    example: "ado workitem comment 42 'Fixed in !17; deploying tomorrow'",
    run: workitem_comment,
}

// ---------- ado workitem link ----------

#[derive(clap::Args)]
pub struct LinkArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// The repository, by name
    #[arg(long)]
    repo: String,
    /// Default {id}-{title-slug}; made from the default branch if missing
    #[arg(long)]
    branch: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BranchLinked {
    work_item: i64,
    repo: String,
    branch: String,
    /// The branch did not exist and was made.
    branch_created: bool,
    /// The work item already carried the link; nothing was written for it.
    already_linked: bool,
}

/// The branch a work item's own work goes on when nobody names one:
/// `{id}-{slug}`, the slug being the title lowercased with every run of
/// characters that are not ASCII letters or digits made one `-`, at most
/// forty characters, so `#715 Fix the thing!` is `715-fix-the-thing`.
fn branch_name(id: i64, title: &str) -> String {
    let mut slug = String::new();
    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.chars().take(40).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        id.to_string()
    } else {
        format!("{id}-{slug}")
    }
}

/// The commit `branch` points at in `repo`, if it exists. The refs listing
/// filters by prefix, so `heads/main` also lists `main-old`; only the exact
/// name counts.
fn branch_head(ctx: &Ctx, ado: &Ado, repo: &RepoRef, branch: &str) -> Result<Option<String>> {
    let full = format!("refs/heads/{branch}");
    let url = ado.code(
        &format!("git/repositories/{}/refs", repo.id),
        &format!("filter={}", query_value(&format!("heads/{branch}"))),
    );
    let answer = ado.get(ctx, &url)?;
    Ok(list(&answer["value"])
        .iter()
        .find(|entry| entry["name"].as_str() == Some(full.as_str()))
        .and_then(|entry| text(&entry["objectId"])))
}

/// Appends one artifact link to work item `id` behind a test of the revision
/// it was read at, unless it already holds a link to the same thing (however
/// Azure DevOps encoded it). Returns whether it wrote one.
pub(crate) fn add_artifact_link(
    ctx: &Ctx,
    ado: &Ado,
    id: i64,
    url: &str,
    name: &str,
) -> Result<bool> {
    let item_url = ado.api(
        None,
        &format!("wit/workitems/{id}"),
        "$expand=relations",
        crate::client::API,
    );
    let item = ado.get(ctx, &item_url)?;
    let rev = item["rev"]
        .as_i64()
        .context("the work item came back without a revision to test")?;
    let wanted = artifact(url);
    if relations(&item).any(|(rel, held)| rel == "ArtifactLink" && artifact(held) == wanted) {
        return Ok(false);
    }
    let document = vec![
        json!({"op": "test", "path": "/rev", "value": rev}),
        json!({"op": "add", "path": "/relations/-", "value": {
            "rel": "ArtifactLink", "url": url, "attributes": {"name": name},
        }}),
    ];
    let url = ado.api(None, &format!("wit/workitems/{id}"), "", crate::client::API);
    ado.patch_work_item(ctx, Method::Patch, &url, document)
        .map_err(|error| moved_on(error, id))?;
    Ok(true)
}

fn workitem_link(ctx: &Ctx, args: LinkArgs) -> Result<BranchLinked> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let repo = ado.repo(ctx, &args.repo)?;
    let branch = match &args.branch {
        Some(branch) => short_branch(branch.trim()),
        None => {
            let url = ado.api(
                None,
                &format!("wit/workitems/{}", id),
                "fields=System.Title",
                crate::client::API,
            );
            let item = ado.get(ctx, &url)?;
            branch_name(
                id,
                &text(&item["fields"]["System.Title"]).unwrap_or_default(),
            )
        }
    };
    let mut branch_created = false;
    if branch_head(ctx, &ado, &repo, &branch)?.is_none() {
        let from = short_branch(repo.default_branch.as_deref().ok_or_else(|| {
            Failure::usage(format!(
                "{} has no default branch to branch from",
                repo.name
            ))
        })?);
        let sha = branch_head(ctx, &ado, &repo, &from)?.with_context(|| {
            format!("there is no branch {from} in {} to branch from", repo.name)
        })?;
        let url = ado.code(&format!("git/repositories/{}/refs", repo.id), "");
        let made = ado.change(
            ctx,
            Effect::Write,
            Method::Post,
            &url,
            json!([{
                "name": format!("refs/heads/{branch}"),
                "oldObjectId": "0000000000000000000000000000000000000000",
                "newObjectId": sha,
            }]),
        )?;
        if made["value"][0]["updateStatus"].as_str() != Some("succeeded") {
            anyhow::bail!(
                "Azure DevOps did not create {branch}: {}",
                made["value"][0]["updateStatus"]
                    .as_str()
                    .unwrap_or("no answer")
            );
        }
        branch_created = true;
    }
    let project_id = repo
        .project_id
        .as_deref()
        .context("the repository came back without its project id")?;
    // The slashes inside a branch name are encoded like the separators.
    let url = format!(
        "vstfs:///Git/Ref/{project_id}%2F{}%2FGB{}",
        repo.id,
        branch.replace('/', "%2F")
    );
    let linked = add_artifact_link(ctx, &ado, id, &url, "Branch")?;
    Ok(BranchLinked {
        work_item: id,
        repo: repo.name,
        branch,
        branch_created,
        already_linked: !linked,
    })
}

command! {
    pub WORKITEM_LINK = ["ado", "workitem", "link"], Write,
    "Link a work item to a branch, creating the branch when missing",
    keywords: ["branch", "start", "work", "on", "connect", "git"],
    example: "ado workitem link 42 --repo web --branch 42-fix-login",
    run: workitem_link,
}

// ---------- ado team list ----------

#[derive(clap::Args)]
pub struct TeamListArgs {
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TeamRow {
    name: String,
    id: String,
    description: Option<String>,
}

fn team_list(ctx: &Ctx, args: TeamListArgs) -> Result<Vec<TeamRow>> {
    let ado = Ado::load(ctx)?;
    let url = ado.api(
        None,
        &format!("projects/{}/teams", segment(&ado.project)),
        &format!("$top={}", args.limit + 1),
        crate::client::API,
    );
    let answer = ado.get(ctx, &url)?;
    let mut teams: Vec<TeamRow> = list(&answer["value"])
        .iter()
        .filter_map(|team| {
            Some(TeamRow {
                name: text(&team["name"])?,
                id: text(&team["id"])?,
                description: text(&team["description"]),
            })
        })
        .collect();
    if teams.len() > args.limit {
        teams.truncate(args.limit);
        ctx.note(format!("[first {}; --limit N for more]", args.limit));
    }
    Ok(teams)
}

command! {
    pub TEAM_LIST = ["ado", "team", "list"], Read,
    "List the project's teams (for [ado] team, which @current needs)",
    keywords: ["teams", "squad", "group", "sprint", "iteration", "setup"],
    example: "ado team list --fields name",
    run: team_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use super::branch_name;
    use crate::testkit::{ado, ado_piped, ado_with, dry_run, urls};

    const BASE: &str = "https://dev.azure.com/contoso";

    fn item(id: i64, rev: i64, title: &str) -> Value {
        json!({"id": id, "rev": rev, "fields": {
            "System.Id": id,
            "System.WorkItemType": "Bug",
            "System.Title": title,
            "System.State": "Active",
            "System.AssignedTo": {"displayName": "Jane Doe", "uniqueName": "jane@contoso.com"},
            "System.IterationPath": "Fabrikam\\Sprint 12",
            "System.AreaPath": "Fabrikam\\Web",
            "Microsoft.VSTS.Common.Priority": 2,
            "System.Tags": "ui; p1",
            "System.ChangedDate": "2026-09-28T10:00:00.000Z"
        }})
    }

    fn wiql(ids: &[i64]) -> Answer {
        let items: Vec<Value> = ids.iter().map(|id| json!({"id": id, "url": "x"})).collect();
        Answer::json(&json!({"queryType": "flat", "workItems": items}))
    }

    fn batch(items: Vec<Value>) -> Answer {
        Answer::json(&json!({"count": items.len(), "value": items}))
    }

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
            vec![wiql(&[])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.stdout.trim(), "[]");
        assert_eq!(
            urls(&transport),
            [format!(
                "{BASE}/Fabrikam/Web%20Team/_apis/wit/wiql?$top=20000&api-version=7.1"
            )],
            "no batch for no ids"
        );
        let query = query_of(&transport, 0);
        assert!(
            query.contains("[System.AssignedTo] = 'Sam O''Neil'"),
            "{query}"
        );
        assert!(
            query.contains("[System.IterationPath] = @CurrentIteration"),
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
    fn more_than_two_hundred_ids_are_read_in_batches_the_endpoint_accepts() {
        let ids: Vec<i64> = (1..=250).collect();
        let first: Vec<Value> = ids[..200].iter().map(|id| item(*id, 1, "t")).collect();
        let second: Vec<Value> = ids[200..].iter().map(|id| item(*id, 1, "t")).collect();
        let (outcome, transport) = ado(
            &[
                "ado", "workitem", "list", "--limit", "250", "--fields", "id",
            ],
            vec![wiql(&ids), batch(first), batch(second)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(outcome.stderr.is_empty(), "{}", outcome.stderr);
        assert_eq!(outcome.json()[249], json!({"id": 250}));
        let sent = transport.sent();
        let count = |at: usize| {
            sent[at].body.as_ref().unwrap()["ids"]
                .as_array()
                .unwrap()
                .len()
        };
        assert_eq!((count(1), count(2)), (200, 50));
    }

    #[test]
    fn get_shows_markdown_links_by_repo_name_and_the_latest_comments() {
        let mut work = item(42, 7, "Login fails on Safari");
        work["fields"]["System.Description"] =
            json!("<p>Steps:</p><ol><li>Open <b>login</b></li></ol>");
        work["fields"]["Microsoft.VSTS.Common.AcceptanceCriteria"] =
            json!("<ul><li>works</li></ul>");
        work["relations"] = json!([
            {"rel": "System.LinkTypes.Hierarchy-Reverse", "url": format!("{BASE}/_apis/wit/workItems/7")},
            {"rel": "System.LinkTypes.Hierarchy-Forward", "url": format!("{BASE}/_apis/wit/workItems/43")},
            {"rel": "System.LinkTypes.Hierarchy-Forward", "url": format!("{BASE}/_apis/wit/workItems/44")},
            {"rel": "System.LinkTypes.Related", "url": format!("{BASE}/_apis/wit/workItems/9")},
            {"rel": "ArtifactLink", "url": "vstfs:///Git/PullRequestId/p-1%2Fr-1%2F17"},
            {"rel": "ArtifactLink", "url": "vstfs:///Git/Ref/p-1%2Fr-1%2FGB42-fix%2Fsafari"},
            {"rel": "ArtifactLink", "url": "vstfs:///Build/Build/991"}
        ]);
        let (outcome, transport) = ado(
            &["ado", "workitem", "get", "42", "--comments", "2"],
            vec![
                Answer::json(&work),
                repos(),
                Answer::json(&json!({"totalCount": 12, "count": 2, "comments": [
                    {"id": 501, "text": "<div>Repro on <code>17.2</code></div>",
                     "createdBy": {"displayName": "Sam Lee"}, "createdDate": "2026-09-28T09:00:00Z"},
                    {"id": 500, "text": "<p></p>", "createdBy": {"displayName": "Sam Lee"},
                     "createdDate": "2026-09-27T09:00:00Z"}
                ]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["title"], "Login fails on Safari");
        assert_eq!(got["rev"], 7);
        assert_eq!(got["parent"], 7);
        assert_eq!(got["children"], json!([43, 44]));
        assert_eq!(got["related"], json!([9]));
        assert_eq!(got["pull_requests"], json!([{"repo": "web", "id": 17}]));
        assert_eq!(
            got["branches"],
            json!([{"repo": "web", "name": "42-fix/safari"}])
        );
        assert_eq!(got["description"], "Steps:\n\n1. Open **login**");
        assert_eq!(got["acceptance_criteria"], "- works");
        assert_eq!(got["comment_count"], 12);
        assert_eq!(
            got["comments"],
            json!([{"id": 501, "author": "Sam Lee", "date": "2026-09-28T09:00:00Z", "text": "Repro on `17.2`"}])
        );
        assert_eq!(got["url"], format!("{BASE}/Fabrikam/_workitems/edit/42"));
        assert_eq!(
            urls(&transport),
            [
                format!("{BASE}/_apis/wit/workitems/42?$expand=relations&api-version=7.1"),
                format!("{BASE}/Fabrikam/_apis/git/repositories?api-version=7.1"),
                format!(
                    "{BASE}/Fabrikam/_apis/wit/workItems/42/comments?$top=2&order=desc&api-version=7.1-preview.4"
                ),
            ]
        );
    }

    #[test]
    fn get_of_a_missing_item_is_4_and_a_sign_in_page_is_3() {
        let (outcome, transport) = ado(
            &["ado", "workitem", "get", "41", "--comments", "0"],
            vec![Answer::json(&item(41, 1, "No links"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent().len(),
            1,
            "no comments asked, no links to name"
        );

        let (outcome, _) = ado(
            &["ado", "workitem", "get", "404"],
            vec![Answer::status(
                404,
                r#"{"message":"TF401232: Work item 404 does not exist."}"#,
            )],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(outcome.stderr.contains("TF401232"), "{}", outcome.stderr);

        let (outcome, _) = ado(
            &["ado", "workitem", "get", "42"],
            vec![Answer::status(203, "<html><body>Sign in</body></html>")],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: run `az login`, or set AZURE_DEVOPS_EXT_PAT"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn create_plans_a_patch_document_with_markdown_as_html_and_the_parent_link() {
        let me = Answer::json(&json!({"authenticatedUser": {"id": "u-1",
            "providerDisplayName": "Jane Doe", "properties": {"Account": {"$value": "jane@contoso.com"}}}}));
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "create",
                "--type",
                "User Story",
                "--title",
                " Pay by card ",
                "--assignee",
                "@me",
                "--parent",
                "7",
                "--tags",
                "ui,UI, p1",
                "--description",
                "**Why**: customers ask",
            ],
            vec![me],
        );
        assert_eq!(plans.len(), 1);
        let plan = &plans[0];
        assert_eq!(plan["method"], "POST");
        assert_eq!(
            plan["url"],
            format!("{BASE}/Fabrikam/_apis/wit/workitems/$User%20Story?api-version=7.1")
        );
        assert_eq!(
            plan["headers"]["Content-Type"],
            "application/json-patch+json"
        );
        assert_eq!(
            plan["body"],
            json!([
                {"op": "add", "path": "/fields/System.Title", "value": "Pay by card"},
                {"op": "add", "path": "/fields/System.AssignedTo", "value": "jane@contoso.com"},
                {"op": "add", "path": "/fields/System.Tags", "value": "ui; p1"},
                {"op": "add", "path": "/fields/System.Description", "value": "<p><b>Why</b>: customers ask</p>"},
                {"op": "add", "path": "/relations/-", "value": {
                    "rel": "System.LinkTypes.Hierarchy-Reverse",
                    "url": format!("{BASE}/_apis/wit/workItems/7")}}
            ])
        );
        assert!(!plan.to_string().contains("fixture-pat"));

        let (outcome, transport) = ado(
            &[
                "ado", "workitem", "create", "--type", "Bug", "--title", "Crash",
            ],
            vec![Answer::json(&item(77, 1, "Crash"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["id"], 77);
        assert!(!transport.sent()[0].method.is_read());
    }

    #[test]
    fn update_leads_with_the_rev_test_and_a_moved_on_item_is_a_conflict() {
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "update",
                "42",
                "--state",
                "Resolved",
                "--assignee",
                "",
                "--if-rev",
                "7",
            ],
            vec![],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            format!("{BASE}/_apis/wit/workitems/42?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!([
                {"op": "test", "path": "/rev", "value": 7},
                {"op": "add", "path": "/fields/System.State", "value": "Resolved"},
                {"op": "remove", "path": "/fields/System.AssignedTo"}
            ])
        );

        for refusal in [
            Answer::status(
                400,
                r#"{"message":"TF401289: The test operation failed for path /rev: expected 7, found 9."}"#,
            ),
            Answer::status(412, r#"{"message":"precondition failed"}"#),
        ] {
            let (outcome, _) = ado(
                &[
                    "ado", "workitem", "update", "42", "--title", "New", "--if-rev", "7",
                ],
                vec![refusal],
            );
            assert_eq!(outcome.code, 5, "{outcome:?}");
            assert!(
                outcome
                    .stderr
                    .starts_with("error: work item 42 changed since it was read"),
                "{}",
                outcome.stderr
            );
            assert!(
                outcome
                    .stderr
                    .contains("hint: re-read it (agent-cli ado workitem get 42"),
                "{}",
                outcome.stderr
            );
        }

        let (outcome, _) = ado(
            &["ado", "workitem", "update", "42", "--state", "Nope"],
            vec![Answer::status(
                400,
                r#"{"message":"TF401320: Rule error for field State."}"#,
            )],
        );
        assert_eq!(
            outcome.code, 2,
            "any other refusal keeps its code: {outcome:?}"
        );

        let (outcome, transport) = ado(&["ado", "workitem", "update", "42"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("nothing to change"));
        assert!(transport.sent().is_empty());

        let (outcome, _) = ado(
            &["ado", "workitem", "update", "42", "--priority", "1"],
            vec![Answer::json(&item(42, 8, "Crash"))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["rev"], 8);
    }

    #[test]
    fn comment_posts_markdown_as_html_to_the_preview_endpoint() {
        let plans = dry_run(
            &["ado", "workitem", "comment", "42", "Fixed in **!17**"],
            vec![],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{BASE}/Fabrikam/_apis/wit/workItems/42/comments?api-version=7.1-preview.4")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"text": "<p>Fixed in <b>!17</b></p>"})
        );

        let (outcome, _) = ado(
            &["ado", "workitem", "comment", "42", "done"],
            vec![Answer::json(
                &json!({"id": 9001, "workItemId": 42, "createdDate": "2026-09-29T10:00:00Z"}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"work_item": 42, "id": 9001, "date": "2026-09-29T10:00:00Z"})
        );
        let (outcome, _) = ado(&["ado", "workitem", "comment", "42", " "], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn long_text_comes_typed_piped_with_a_dash_or_from_a_file() {
        let (outcome, transport) = ado_piped(
            "## Why\n\n- customers ask\n",
            &[
                "ado",
                "workitem",
                "create",
                "--type",
                "Bug",
                "--title",
                "Crash",
                "--description",
                "-",
                "--dry-run",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(transport.sent().is_empty());
        assert_eq!(
            outcome.json()["would"][0]["body"][1],
            json!({"op": "add", "path": "/fields/System.Description",
                "value": markdown_html("## Why\n\n- customers ask")})
        );

        let dir = tempfile::tempdir().unwrap();
        let criteria = dir.path().join("criteria.md");
        std::fs::write(&criteria, "- works on Safari\n").unwrap();
        let criteria = criteria.to_str().unwrap();
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "update",
                "42",
                "--description",
                "- a bullet, not a flag",
                "--acceptance-criteria-file",
                criteria,
            ],
            vec![],
        );
        assert_eq!(
            plans[0]["body"],
            json!([
                {"op": "add", "path": "/fields/System.Description", "value": markdown_html("- a bullet, not a flag")},
                {"op": "add", "path": "/fields/Microsoft.VSTS.Common.AcceptanceCriteria", "value": markdown_html("- works on Safari")}
            ])
        );

        for (stdin, argv, said) in [
            (
                "x",
                &[
                    "ado",
                    "workitem",
                    "update",
                    "42",
                    "--description",
                    "-",
                    "--acceptance-criteria",
                    "-",
                ][..],
                "cannot both read stdin",
            ),
            (
                "x",
                &[
                    "ado",
                    "workitem",
                    "update",
                    "42",
                    "--description",
                    "x",
                    "--description-file",
                    criteria,
                ][..],
                "both as a value and as the file",
            ),
            (
                "",
                &["ado", "workitem", "update", "42", "--description", "-"][..],
                "the description from stdin is empty",
            ),
            (
                "x",
                &[
                    "ado",
                    "workitem",
                    "update",
                    "42",
                    "--acceptance-criteria-file",
                    "/nonexistent.md",
                ][..],
                "cannot read the acceptance criteria from /nonexistent.md",
            ),
        ] {
            let (outcome, transport) = ado_piped(stdin, argv, vec![]);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(outcome.stderr.contains(said), "{}", outcome.stderr);
            assert!(transport.sent().is_empty());
        }
    }

    /// What `markdown_to_html` makes of `markdown`, so the tests above do not
    /// restate the renderer.
    fn markdown_html(markdown: &str) -> String {
        crate::markdown::markdown_to_html(markdown)
    }

    #[test]
    fn a_piped_comment_is_a_code_block_a_file_is_markdown_and_both_stop_at_64_kib() {
        let (outcome, _) = ado_piped(
            "test result: FAILED. 1 passed; 1 failed\n",
            &["ado", "workitem", "comment", "42", "-", "--dry-run"],
            vec![],
        );
        assert_eq!(
            outcome.json()["would"][0]["body"],
            json!({"text": "<pre>test result: FAILED. 1 passed; 1 failed</pre>"})
        );

        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("note.md");
        std::fs::write(&note, "Fixed in **!17**\n").unwrap();
        let plans = dry_run(
            &[
                "ado",
                "workitem",
                "comment",
                "42",
                "--text-file",
                note.to_str().unwrap(),
            ],
            vec![],
        );
        assert_eq!(
            plans[0]["body"],
            json!({"text": "<p>Fixed in <b>!17</b></p>"})
        );

        let log = "x".repeat(64 * 1024 + 1);
        std::fs::write(&note, &log).unwrap();
        for (stdin, argv) in [
            (log.as_str(), &["ado", "workitem", "comment", "42", "-"][..]),
            (
                "",
                &[
                    "ado",
                    "workitem",
                    "comment",
                    "42",
                    "--text-file",
                    note.to_str().unwrap(),
                ][..],
            ),
        ] {
            let (outcome, transport) = ado_piped(stdin, argv, vec![]);
            assert_eq!(outcome.code, 2, "{outcome:?}");
            assert!(
                outcome.stderr.contains("more than 64 KiB"),
                "{}",
                outcome.stderr
            );
            assert!(transport.sent().is_empty());
        }
        let (outcome, _) = ado(&["ado", "workitem", "comment", "42"], vec![]);
        assert_eq!(outcome.code, 2, "text or --text-file: {outcome:?}");
    }

    fn repos() -> Answer {
        Answer::json(&json!({"count": 1, "value": [{"id": "r-1", "name": "web",
            "defaultBranch": "refs/heads/main", "project": {"id": "p-1", "name": "Fabrikam"}}]}))
    }

    fn refs(name: &str, sha: &str) -> Answer {
        Answer::json(&json!({"count": 1, "value": [{"name": name, "objectId": sha}]}))
    }

    #[test]
    fn link_makes_the_missing_branch_from_the_default_one_before_linking() {
        let title = Answer::json(
            &json!({"id": 42, "rev": 3, "fields": {"System.Title": "Fix the login!"}}),
        );
        let plans = dry_run(
            &["ado", "workitem", "link", "42", "--repo", "WEB"],
            vec![
                repos(),
                title,
                // The prefix filter answers with a different branch only.
                refs("refs/heads/42-fix-the-login-old", "aaa"),
                refs("refs/heads/main", "c0ffee"),
            ],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{BASE}/Fabrikam/_apis/git/repositories/r-1/refs?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!([{"name": "refs/heads/42-fix-the-login",
                "oldObjectId": "0000000000000000000000000000000000000000", "newObjectId": "c0ffee"}])
        );
    }

    #[test]
    fn link_to_an_existing_branch_patches_the_work_item_once_and_not_twice() {
        let mut current = item(42, 3, "Fix");
        let (outcome, transport) = ado(
            &[
                "ado",
                "workitem",
                "link",
                "42",
                "--repo",
                "web",
                "--branch",
                "refs/heads/feature/login",
            ],
            vec![
                repos(),
                refs("refs/heads/feature/login", "abc"),
                Answer::json(&current),
                Answer::json(&item(42, 4, "Fix")),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"work_item": 42, "repo": "web", "branch": "feature/login",
                "branch_created": false, "already_linked": false})
        );
        let patch = &transport.sent()[3];
        assert_eq!(
            patch.url,
            format!("{BASE}/_apis/wit/workitems/42?api-version=7.1")
        );
        assert_eq!(
            patch.body.as_ref().unwrap(),
            &json!([
                {"op": "test", "path": "/rev", "value": 3},
                {"op": "add", "path": "/relations/-", "value": {"rel": "ArtifactLink",
                    "url": "vstfs:///Git/Ref/p-1%2Fr-1%2FGBfeature%2Flogin",
                    "attributes": {"name": "Branch"}}}
            ])
        );

        current["relations"] = json!([{"rel": "ArtifactLink",
            "url": "vstfs:///Git/Ref/p-1%2fr-1%2fGBfeature%2flogin"}]);
        let (outcome, transport) = ado(
            &[
                "ado",
                "workitem",
                "link",
                "42",
                "--repo",
                "web",
                "--branch",
                "feature/login",
            ],
            vec![
                repos(),
                refs("refs/heads/feature/login", "abc"),
                Answer::json(&current),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["already_linked"], true);
        assert!(transport.sent().iter().all(|sent| sent.method.is_read()));

        let (outcome, _) = ado(
            &["ado", "workitem", "link", "42", "--repo", "api"],
            vec![repos(), repos()],
        );
        assert_eq!(
            outcome.code, 4,
            "an unknown repo is looked up afresh, then not found: {outcome:?}"
        );
        assert!(
            outcome.stderr.contains("hint: agent-cli ado repo list"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn team_list_names_the_teams_and_says_when_there_are_more() {
        let (outcome, transport) = ado(
            &["ado", "team", "list", "--limit", "1"],
            vec![Answer::json(&json!({"count": 2, "value": [
                {"id": "t-1", "name": "Web Team", "description": "The web app"},
                {"id": "t-2", "name": "Data", "description": ""}
            ]}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "Web Team", "id": "t-1", "description": "The web app"}])
        );
        assert_eq!(outcome.stderr, "[first 1; --limit N for more]\n");
        assert_eq!(
            urls(&transport),
            [format!(
                "{BASE}/_apis/projects/Fabrikam/teams?$top=2&api-version=7.1"
            )]
        );
    }

    #[test]
    fn a_branch_name_is_the_id_and_a_forty_character_slug() {
        assert_eq!(branch_name(715, "Fix the thing!"), "715-fix-the-thing");
        assert_eq!(
            branch_name(
                715,
                "  Réécrire — the (whole) sync/path, again & again & again "
            ),
            "715-r-crire-the-whole-sync-path-again-again"
        );
        assert_eq!(branch_name(715, "???"), "715");
    }
}
