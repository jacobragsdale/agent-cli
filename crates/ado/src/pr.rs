//! Repositories and pull requests. Every pull request the filters match is
//! listed: ticket-tui showed only repositories with a clone on the machine.

use agent_cli_core::{Ctx, Effect, Exit, Failure, Method, When, command};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{
    Ado, Kind, PREVIEW_API, full_ref, list, query_value, segment, short_branch, stamp, text,
};
use crate::markdown::CommentBody;
use crate::workitem::add_artifact_link;

// ---------- ado repo list ----------

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

// ---------- ado repo get ----------

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

// ---------- pull request rows ----------

/// One pull request as a list shows it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PrRow {
    id: i64,
    repo: Option<String>,
    title: Option<String>,
    author: Option<String>,
    /// active, completed or abandoned.
    status: Option<String>,
    is_draft: bool,
    source: Option<String>,
    target: Option<String>,
    /// succeeded, conflicts, queued …
    merge_status: Option<String>,
    /// Who set auto-complete, when it is on.
    auto_complete: Option<String>,
    created: Option<String>,
    reviewers: Vec<Reviewer>,
    url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Reviewer {
    name: String,
    /// approved, suggestions, none, waiting or rejected.
    vote: &'static str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    required: bool,
}

/// A vote on the API's own scale, in words.
fn vote_word(vote: i64) -> &'static str {
    match vote {
        10 => "approved",
        5 => "suggestions",
        -5 => "waiting",
        -10 => "rejected",
        _ => "none",
    }
}

fn pr_row(ado: &Ado, pr: &Value) -> PrRow {
    let id = pr["pullRequestId"].as_i64().unwrap_or_default();
    let repo = text(&pr["repository"]["name"]);
    let project =
        text(&pr["repository"]["project"]["name"]).unwrap_or_else(|| ado.code_project.clone());
    PrRow {
        id,
        url: repo
            .as_deref()
            .map(|repo| ado.pull_request_url(&project, repo, id)),
        repo,
        title: text(&pr["title"]),
        author: text(&pr["createdBy"]["displayName"]),
        status: text(&pr["status"]),
        is_draft: pr["isDraft"].as_bool().unwrap_or_default(),
        source: pr["sourceRefName"].as_str().map(short_branch),
        target: pr["targetRefName"].as_str().map(short_branch),
        merge_status: text(&pr["mergeStatus"]),
        auto_complete: text(&pr["autoCompleteSetBy"]["displayName"]),
        created: stamp(&pr["creationDate"]),
        reviewers: list(&pr["reviewers"])
            .iter()
            .filter_map(|reviewer| {
                Some(Reviewer {
                    name: text(&reviewer["displayName"])?,
                    vote: vote_word(reviewer["vote"].as_i64().unwrap_or_default()),
                    required: reviewer["isRequired"].as_bool().unwrap_or_default(),
                })
            })
            .collect(),
    }
}

// ---------- ado pr list ----------

#[derive(Clone, Copy, clap::ValueEnum)]
enum Status {
    Active,
    Completed,
    Abandoned,
    All,
}

#[derive(clap::Args)]
pub struct PrListArgs {
    /// The repository, by name
    #[arg(long)]
    repo: Option<String>,
    /// Which pull requests
    #[arg(long, value_enum, default_value = "active")]
    status: Status,
    /// Who opened it: name, email or @me
    #[arg(long)]
    author: Option<String>,
    /// A reviewer: name, email or @me
    #[arg(long)]
    reviewer: Option<String>,
    /// The branch it merges into
    #[arg(long)]
    target: Option<String>,
    /// The branch it merges from
    #[arg(long)]
    source: Option<String>,
    /// True for drafts only, false for none
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    draft: Option<bool>,
    /// Opened after this
    #[arg(long)]
    since: Option<When>,
    /// Opened before this
    #[arg(long)]
    until: Option<When>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A page of the search; more is read only while fewer than `--limit + 1`
/// rows have matched (drafts are filtered here, the rest by Azure DevOps).
const PR_PAGE: usize = 100;

fn pr_list(ctx: &Ctx, args: PrListArgs) -> Result<Vec<PrRow>> {
    let ado = Ado::load(ctx)?;
    let status = match args.status {
        Status::Active => "active",
        Status::Completed => "completed",
        Status::Abandoned => "abandoned",
        Status::All => "all",
    };
    let mut criteria = format!("searchCriteria.status={status}");
    if let Some(who) = &args.author {
        criteria.push_str(&format!(
            "&searchCriteria.creatorId={}",
            ado.identity(ctx, who)?
        ));
    }
    if let Some(who) = &args.reviewer {
        criteria.push_str(&format!(
            "&searchCriteria.reviewerId={}",
            ado.identity(ctx, who)?
        ));
    }
    for (key, branch) in [
        ("targetRefName", &args.target),
        ("sourceRefName", &args.source),
    ] {
        if let Some(branch) = branch {
            criteria.push_str(&format!(
                "&searchCriteria.{key}={}",
                query_value(&full_ref(branch))
            ));
        }
    }
    for (key, when) in [("minTime", args.since), ("maxTime", args.until)] {
        if let Some(when) = when {
            criteria.push_str(&format!(
                "&searchCriteria.{key}={}",
                query_value(&when.utc())
            ));
        }
    }
    let path = match &args.repo {
        Some(repo) => format!("git/repositories/{}/pullrequests", segment(repo)),
        None => "git/pullrequests".to_owned(),
    };
    let page = (args.limit + 1).min(PR_PAGE);
    let mut rows = Vec::new();
    let mut skip = 0;
    loop {
        let url = ado.code(&path, &format!("{criteria}&$top={page}&$skip={skip}"));
        let answer = ado.get(ctx, &url)?;
        let found = list(&answer["value"]);
        rows.extend(
            found
                .iter()
                .filter(|pr| {
                    args.draft
                        .is_none_or(|draft| pr["isDraft"].as_bool().unwrap_or_default() == draft)
                })
                .map(|pr| pr_row(&ado, pr)),
        );
        if rows.len() > args.limit || found.len() < page {
            break;
        }
        skip += page;
    }
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!(
            "[first {}; more match: --limit N, or narrow with --repo --author --target]",
            args.limit
        ));
    }
    Ok(rows)
}

command! {
    pub PR_LIST = ["ado", "pr", "list"], Read,
    "List pull requests by repo, author, reviewer, branch or status",
    keywords: ["open", "active", "mine", "reviewer", "pending", "waiting", "drafts"],
    example: "ado pr list --reviewer @me --fields id,title,author,repo",
    run: pr_list,
}

// ---------- ado pr get ----------

#[derive(clap::Args)]
pub struct PrGetArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// How many of the latest open threads to include
    #[arg(long, default_value_t = 5)]
    comments: usize,
}

/// One pull request with who voted what, what it closes, what its policies
/// say and the latest open discussion.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PullRequest {
    #[serde(flatten)]
    row: PrRow,
    /// Markdown.
    description: Option<String>,
    work_items: Vec<i64>,
    /// Branch policies: builds, reviewer counts, linked work items …
    policies: Vec<Policy>,
    /// Threads still active or pending.
    open_threads: usize,
    /// The latest open threads first.
    threads: Vec<Thread>,
    /// The source commit the merge would take.
    last_merge_source_commit: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Policy {
    name: Option<String>,
    /// approved, rejected, running, queued …
    status: Option<String>,
    /// The build it ran, for a build policy.
    run_id: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Thread {
    id: i64,
    status: Option<String>,
    author: Option<String>,
    /// The file and line a code comment is on.
    file: Option<String>,
    line: Option<i64>,
    date: Option<String>,
    /// The first comment, Markdown.
    text: Option<String>,
    replies: usize,
}

/// The pull request as stored, found by id alone: the project-wide endpoint
/// needs no repository.
fn fetch_pr(ctx: &Ctx, ado: &Ado, id: i64) -> Result<Value> {
    ado.get(ctx, &ado.code(&format!("git/pullrequests/{id}"), ""))
}

/// The repository id and project id a pull request lives under.
fn pr_home(pr: &Value) -> Result<(String, String)> {
    let repo = text(&pr["repository"]["id"])
        .context("the pull request came back without its repository")?;
    let project = text(&pr["repository"]["project"]["id"])
        .context("the pull request came back without its project")?;
    Ok((repo, project))
}

/// The work items a pull request says it closes.
fn pr_work_items(ctx: &Ctx, ado: &Ado, repo_id: &str, id: i64) -> Result<Vec<i64>> {
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullRequests/{id}/workitems"),
        "",
    );
    let answer = ado.get(ctx, &url)?;
    Ok(list(&answer["value"])
        .iter()
        .filter_map(|entry| {
            entry["id"]
                .as_str()
                .and_then(|id| id.parse().ok())
                .or_else(|| entry["id"].as_i64())
        })
        .collect())
}

fn pr_get(ctx: &Ctx, args: PrGetArgs) -> Result<PullRequest> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let pr = fetch_pr(ctx, &ado, id)?;
    let (repo_id, project_id) = pr_home(&pr)?;
    let work_items = pr_work_items(ctx, &ado, &repo_id, id)?;
    let evaluations = ado.get(
        ctx,
        &ado.api(
            Some(&ado.code_project),
            "policy/evaluations",
            &format!(
                "artifactId={}",
                query_value(&format!(
                    "vstfs:///CodeReview/CodeReviewId/{project_id}/{}",
                    id
                ))
            ),
            PREVIEW_API,
        ),
    )?;
    let policies = list(&evaluations["value"])
        .iter()
        .filter(|evaluation| evaluation["status"].as_str() != Some("notApplicable"))
        .map(|evaluation| {
            let configuration = &evaluation["configuration"];
            Policy {
                name: text(&configuration["settings"]["displayName"])
                    .or_else(|| text(&configuration["type"]["displayName"])),
                status: text(&evaluation["status"]),
                run_id: evaluation["context"]["buildId"].as_i64(),
            }
        })
        .collect();
    let threads = ado.get(
        ctx,
        &ado.code(
            &format!("git/repositories/{repo_id}/pullRequests/{}/threads", id),
            "",
        ),
    )?;
    let mut open: Vec<&Value> = list(&threads["value"])
        .iter()
        .filter(|thread| {
            matches!(thread["status"].as_str(), Some("active" | "pending"))
                && !thread["isDeleted"].as_bool().unwrap_or_default()
                // A thread Azure DevOps wrote about itself (a vote, a push)
                // is not discussion.
                && thread["comments"][0]["commentType"].as_str() != Some("system")
        })
        .collect();
    open.sort_by(|a, b| {
        b["lastUpdatedDate"]
            .as_str()
            .cmp(&a["lastUpdatedDate"].as_str())
    });
    let open_threads = open.len();
    let threads = open
        .into_iter()
        .take(args.comments)
        .map(|thread| {
            let first = &thread["comments"][0];
            let context = &thread["threadContext"];
            Thread {
                id: thread["id"].as_i64().unwrap_or_default(),
                status: text(&thread["status"]),
                author: text(&first["author"]["displayName"]),
                file: text(&context["filePath"]),
                line: context["rightFileStart"]["line"].as_i64(),
                date: stamp(&thread["lastUpdatedDate"]),
                text: text(&first["content"]),
                replies: list(&thread["comments"]).len().saturating_sub(1),
            }
        })
        .collect();
    Ok(PullRequest {
        row: pr_row(&ado, &pr),
        description: text(&pr["description"]),
        work_items,
        policies,
        open_threads,
        threads,
        last_merge_source_commit: text(&pr["lastMergeSourceCommit"]["commitId"]),
    })
}

command! {
    pub PR_GET = ["ado", "pr", "get"], Read,
    "Show a pull request: reviewers and votes, work items, policies, threads",
    keywords: ["review", "approved", "who", "votes", "checks", "status", "comments", "details"],
    example: "ado pr get 42 --fields title,status,reviewers,policies",
    run: pr_get,
}

// ---------- ado pr create ----------

#[derive(clap::Args)]
pub struct PrCreateArgs {
    /// The repository, by name
    #[arg(long)]
    repo: String,
    /// The branch it merges from
    #[arg(long)]
    source: String,
    /// The branch it merges into (default: the repo's default branch)
    #[arg(long)]
    target: Option<String>,
    /// The pull request's title
    #[arg(long)]
    title: String,
    /// Markdown
    #[arg(long)]
    description: Option<String>,
    /// A work item to link (repeatable)
    #[arg(long)]
    workitem: Vec<i64>,
    /// Open it as a draft
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    draft: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrCreated {
    id: i64,
    url: Option<String>,
    repo: String,
    source: Option<String>,
    target: Option<String>,
    status: Option<String>,
    is_draft: bool,
    /// False when one was already open between these branches and reused.
    created: bool,
    work_items: Vec<i64>,
}

/// Links work item `work_item` to pull request `id`: the link lives on the
/// work item, as an artifact link naming the pull request. Returns whether
/// it wrote one (false: it was already there).
fn link_pr(
    ctx: &Ctx,
    ado: &Ado,
    project_id: &str,
    repo_id: &str,
    id: i64,
    work_item: i64,
) -> Result<bool> {
    let url = format!("vstfs:///Git/PullRequestId/{project_id}%2F{repo_id}%2F{id}");
    add_artifact_link(ctx, ado, work_item, &url, "Pull Request")
}

/// One pull request between two branches, whatever happened last time. None
/// open: one is opened with the links asked for. One open: it is kept as it
/// is and only the links it lacks are added. Two open is a question, not a
/// guess. Azure DevOps does not promise the links land with the pull request,
/// so they are read back, repaired, and read back again.
fn pr_create(ctx: &Ctx, args: PrCreateArgs) -> Result<PrCreated> {
    let ado = Ado::load(ctx)?;
    let title = args.title.trim();
    if title.is_empty() {
        return Err(Failure::usage("a pull request needs a title").into());
    }
    let repo = ado.repo(ctx, &args.repo)?;
    let source = full_ref(&args.source);
    let target = match &args.target {
        Some(target) => full_ref(target),
        None => repo.default_branch.clone().ok_or_else(|| {
            Failure::usage(format!(
                "{} has no default branch; pass --target",
                repo.name
            ))
        })?,
    };
    let url = ado.code(&format!("git/repositories/{}/pullrequests", repo.id), "");
    let open = ado.get(
        ctx,
        &ado.code(
            &format!("git/repositories/{}/pullrequests", repo.id),
            &format!(
                "searchCriteria.status=active&searchCriteria.sourceRefName={}&searchCriteria.targetRefName={}",
                query_value(&source),
                query_value(&target)
            ),
        ),
    )?;
    let (pr, created) = match list(&open["value"]) {
        [] => {
            let body = json!({
                "sourceRefName": source,
                "targetRefName": target,
                "title": title,
                "description": args.description.unwrap_or_default(),
                "isDraft": args.draft.unwrap_or(false),
                "workItemRefs": args.workitem.iter().map(|id| json!({"id": id.to_string()})).collect::<Vec<_>>(),
            });
            (
                ado.change(ctx, Effect::Write, Method::Post, &url, body)?,
                true,
            )
        }
        [one] => (one.clone(), false),
        several => {
            let ids: Vec<String> = several
                .iter()
                .filter_map(|pr| pr["pullRequestId"].as_i64())
                .map(|id| id.to_string())
                .collect();
            return Err(Failure::conflict(format!(
                "{} active pull requests already go from {} into {} in {}: {}",
                several.len(),
                short_branch(&source),
                short_branch(&target),
                repo.name,
                ids.join(", ")
            ))
            .hint("abandon the extra ones (agent-cli ado pr abandon ID --yes), or pass the id to the command you meant")
            .into());
        }
    };
    let id = pr["pullRequestId"]
        .as_i64()
        .context("Azure DevOps answered with a pull request it did not number")?;
    let project_id = repo
        .project_id
        .clone()
        .context("the repository came back without its project id")?;
    let mut linked = pr_work_items(ctx, &ado, &repo.id, id)?;
    let mut repaired = false;
    for work_item in &args.workitem {
        if !linked.contains(work_item) {
            link_pr(ctx, &ado, &project_id, &repo.id, id, *work_item)?;
            repaired = true;
        }
    }
    if repaired {
        linked = pr_work_items(ctx, &ado, &repo.id, id)?;
    }
    let row = pr_row(&ado, &pr);
    let missing: Vec<String> = args
        .workitem
        .iter()
        .filter(|work_item| !linked.contains(work_item))
        .map(ToString::to_string)
        .collect();
    let made = PrCreated {
        id,
        url: row.url,
        repo: repo.name,
        source: row.source,
        target: row.target,
        status: row.status,
        is_draft: row.is_draft,
        created,
        work_items: linked,
    };
    if !missing.is_empty() {
        // The pull request is real either way, so it prints either way.
        return Err(Failure::new(
            Exit::Failed,
            format!(
                "pull request {id} is open, but work item {} did not link",
                missing.join(", ")
            ),
        )
        .hint("run the same command again to repair the links")
        .with_data(made)
        .into());
    }
    Ok(made)
}

command! {
    pub PR_CREATE = ["ado", "pr", "create"], Write,
    "Open a pull request linked to work items, or reuse the one already open",
    keywords: ["new", "open", "raise", "submit", "merge", "request", "branch"],
    example: "ado pr create --repo web --source 42-fix-login --title 'Fix login' --workitem 42",
    run: pr_create,
}

// ---------- ado pr vote ----------

#[derive(Clone, Copy, clap::ValueEnum)]
enum Vote {
    Approve,
    Suggest,
    Wait,
    Reject,
    None,
}

#[derive(clap::Args)]
pub struct VoteArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// Your vote (none withdraws it)
    #[arg(value_enum)]
    vote: Vote,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Voted {
    id: i64,
    /// approved, suggestions, none, waiting or rejected.
    vote: &'static str,
}

fn pr_vote(ctx: &Ctx, args: VoteArgs) -> Result<Voted> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let value: i64 = match args.vote {
        Vote::Approve => 10,
        Vote::Suggest => 5,
        Vote::Wait => -5,
        Vote::Reject => -10,
        Vote::None => 0,
    };
    let pr = fetch_pr(ctx, &ado, id)?;
    let (repo_id, _) = pr_home(&pr)?;
    let me = ado.me(ctx)?;
    // Voting on a pull request you do not review adds you as a reviewer.
    let url = ado.code(
        &format!(
            "git/repositories/{repo_id}/pullrequests/{}/reviewers/{}",
            id, me.id
        ),
        "",
    );
    ado.change(
        ctx,
        Effect::Write,
        Method::Put,
        &url,
        json!({"vote": value}),
    )?;
    Ok(Voted {
        id,
        vote: vote_word(value),
    })
}

command! {
    pub PR_VOTE = ["ado", "pr", "vote"], Write,
    "Record your vote on a pull request: approve, suggest, reject or none",
    keywords: ["approve", "reject", "sign", "off", "lgtm", "reviewer"],
    example: "ado pr vote 42 approve",
    run: pr_vote,
}

// ---------- ado pr update ----------

#[derive(Clone, Copy, clap::ValueEnum)]
enum Toggle {
    On,
    Off,
}

#[derive(clap::Args)]
pub struct PrUpdateArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// Complete it by itself once policies pass
    #[arg(long, value_enum)]
    autocomplete: Option<Toggle>,
    /// True to make it a draft, false to publish it
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    draft: Option<bool>,
    /// A new title
    #[arg(long)]
    title: Option<String>,
    /// Markdown; replaces the description
    #[arg(long)]
    description: Option<String>,
}

fn pr_update(ctx: &Ctx, args: PrUpdateArgs) -> Result<PrRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let mut body = serde_json::Map::new();
    if let Some(draft) = args.draft {
        body.insert("isDraft".into(), draft.into());
    }
    if let Some(title) = &args.title {
        body.insert("title".into(), title.trim().into());
    }
    if let Some(description) = &args.description {
        body.insert("description".into(), description.as_str().into());
    }
    if body.is_empty() && args.autocomplete.is_none() {
        return Err(Failure::usage("nothing to change")
            .hint(format!("pass --autocomplete on|off, --draft true|false, --title or --description, e.g. agent-cli ado pr update {} --autocomplete on", id))
            .into());
    }
    let pr = fetch_pr(ctx, &ado, id)?;
    let (repo_id, _) = pr_home(&pr)?;
    match args.autocomplete {
        Some(Toggle::On) => {
            body.insert("autoCompleteSetBy".into(), json!({"id": ado.me(ctx)?.id}));
        }
        Some(Toggle::Off) => {
            // The empty GUID is how the API is told nobody set it.
            body.insert(
                "autoCompleteSetBy".into(),
                json!({"id": "00000000-0000-0000-0000-000000000000"}),
            );
        }
        None => {}
    }
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullrequests/{}", id),
        "",
    );
    let updated = ado.change(ctx, Effect::Write, Method::Patch, &url, Value::Object(body))?;
    Ok(pr_row(&ado, &updated))
}

command! {
    pub PR_UPDATE = ["ado", "pr", "update"], Write,
    "Turn auto-complete on or off, mark draft or ready, or retitle a pull request",
    keywords: ["autocomplete", "auto", "complete", "draft", "publish", "ready", "rename", "edit"],
    example: "ado pr update 42 --autocomplete on",
    run: pr_update,
}

// ---------- ado pr link ----------

#[derive(clap::Args)]
pub struct PrLinkArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// The work item to link
    #[arg(long)]
    workitem: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrLinked {
    pr: i64,
    work_item: i64,
    /// Nothing was written: the link was already there.
    already_linked: bool,
}

fn pr_link(ctx: &Ctx, args: PrLinkArgs) -> Result<PrLinked> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let pr = fetch_pr(ctx, &ado, id)?;
    let (repo_id, project_id) = pr_home(&pr)?;
    let wrote = link_pr(ctx, &ado, &project_id, &repo_id, id, args.workitem)?;
    Ok(PrLinked {
        pr: id,
        work_item: args.workitem,
        already_linked: !wrote,
    })
}

command! {
    pub PR_LINK = ["ado", "pr", "link"], Write,
    "Link a work item to a pull request",
    keywords: ["attach", "associate", "ticket", "connect", "resolves"],
    example: "ado pr link 17 --workitem 42",
    run: pr_link,
}

// ---------- ado pr comment ----------

#[derive(clap::Args)]
pub struct PrCommentArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// Markdown, or - to read stdin (posted as a code block, 64 KiB max)
    #[arg(allow_hyphen_values = true)]
    text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrCommented {
    pr: i64,
    thread_id: Option<i64>,
}

fn pr_comment(ctx: &Ctx, args: PrCommentArgs) -> Result<PrCommented> {
    let body = CommentBody::from_arg(&args.text)?;
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let pr = fetch_pr(ctx, &ado, id)?;
    let (repo_id, _) = pr_home(&pr)?;
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullRequests/{}/threads", id),
        "",
    );
    let thread = ado.change(
        ctx,
        Effect::Write,
        Method::Post,
        &url,
        json!({
            "comments": [{"parentCommentId": 0, "content": body.markdown(), "commentType": "text"}],
            "status": "active",
        }),
    )?;
    Ok(PrCommented {
        pr: id,
        thread_id: thread["id"].as_i64(),
    })
}

command! {
    pub PR_COMMENT = ["ado", "pr", "comment"], Write,
    "Start a comment thread on a pull request (Markdown, or - for stdin)",
    keywords: ["discussion", "note", "reply", "post", "feedback", "review"],
    example: "ado pr comment 42 'Tests pass locally; ready for review'",
    run: pr_comment,
}

// ---------- ado pr complete / abandon ----------

#[derive(Clone, Copy, clap::ValueEnum)]
enum Strategy {
    Squash,
    Merge,
    Rebase,
}

#[derive(clap::Args)]
pub struct CompleteArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
    /// How it lands on the target
    #[arg(long, value_enum, default_value = "squash")]
    strategy: Strategy,
    /// Keep the source branch instead of deleting it
    #[arg(long)]
    keep_source: bool,
    /// Leave the linked work items' states alone
    #[arg(long)]
    no_transition: bool,
}

/// The pull request as it is now, refusing one that is no longer active.
fn active_pr(ctx: &Ctx, ado: &Ado, id: i64) -> Result<(Value, String)> {
    let pr = fetch_pr(ctx, ado, id)?;
    let status = pr["status"].as_str().unwrap_or_default();
    if status != "active" {
        return Err(
            Failure::conflict(format!("pull request {id} is already {status}"))
                .hint(format!("agent-cli ado pr get {id} --fields status,url"))
                .into(),
        );
    }
    let (repo_id, _) = pr_home(&pr)?;
    Ok((pr, repo_id))
}

fn pr_complete(ctx: &Ctx, args: CompleteArgs) -> Result<PrRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let (pr, repo_id) = active_pr(ctx, &ado, id)?;
    let strategy = match args.strategy {
        Strategy::Squash => "squash",
        Strategy::Merge => "noFastForward",
        Strategy::Rebase => "rebase",
    };
    // The head it was read at goes with it, so a merge that raced someone
    // else's push is refused by Azure DevOps rather than landing over it.
    let body = json!({
        "status": "completed",
        "lastMergeSourceCommit": pr["lastMergeSourceCommit"],
        "completionOptions": {
            "mergeStrategy": strategy,
            "deleteSourceBranch": !args.keep_source,
            "transitionWorkItems": !args.no_transition,
        },
    });
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullrequests/{}", id),
        "",
    );
    let done = ado.change(ctx, Effect::Destructive, Method::Patch, &url, body)?;
    Ok(pr_row(&ado, &done))
}

command! {
    pub PR_COMPLETE = ["ado", "pr", "complete"], Destructive,
    "Complete (merge) a pull request: squash, merge or rebase",
    keywords: ["merge", "land", "finish", "squash", "ship"],
    example: "ado pr complete 42 --strategy squash --yes",
    run: pr_complete,
}

#[derive(clap::Args)]
pub struct AbandonArgs {
    /// The pull request's id: 431, #431 or its web URL
    id: String,
}

fn pr_abandon(ctx: &Ctx, args: AbandonArgs) -> Result<PrRow> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.id)?;
    let (_, repo_id) = active_pr(ctx, &ado, id)?;
    let url = ado.code(
        &format!("git/repositories/{repo_id}/pullrequests/{}", id),
        "",
    );
    let done = ado.change(
        ctx,
        Effect::Destructive,
        Method::Patch,
        &url,
        json!({"status": "abandoned"}),
    )?;
    Ok(pr_row(&ado, &done))
}

command! {
    pub PR_ABANDON = ["ado", "pr", "abandon"], Destructive,
    "Abandon (close) a pull request, discarding its changes",
    keywords: ["close", "without", "merging", "discard", "drop", "cancel"],
    example: "ado pr abandon 42 --yes",
    run: pr_abandon,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testkit::{ado, dry_run, urls};

    const BASE: &str = "https://dev.azure.com/contoso";
    const CODE: &str = "https://dev.azure.com/contoso/Fabrikam/_apis";

    fn pr(id: i64, draft: bool) -> Value {
        json!({
            "pullRequestId": id,
            "repository": {"id": "r-1", "name": "web", "project": {"id": "p-1", "name": "Fabrikam"}},
            "title": format!("Change {id}"),
            "status": "active",
            "isDraft": draft,
            "createdBy": {"displayName": "Jane Doe", "uniqueName": "jane@contoso.com"},
            "creationDate": "2026-09-28T10:00:00Z",
            "sourceRefName": "refs/heads/42-fix-login",
            "targetRefName": "refs/heads/main",
            "mergeStatus": "succeeded",
            "lastMergeSourceCommit": {"commitId": "abc123"},
            "description": "Fixes **login**",
            "reviewers": [
                {"id": "u-2", "displayName": "Sam Lee", "vote": 10, "isRequired": true},
                {"id": "u-3", "displayName": "Web Team", "vote": -5}
            ]
        })
    }

    fn page(items: Vec<Value>) -> Answer {
        Answer::json(&json!({"count": items.len(), "value": items}))
    }

    fn me() -> Answer {
        Answer::json(
            &json!({"authenticatedUser": {"id": "u-1", "providerDisplayName": "Jane Doe"}}),
        )
    }

    fn repos() -> Answer {
        page(vec![
            json!({"id": "r-1", "name": "web", "defaultBranch": "refs/heads/main",
            "project": {"id": "p-1", "name": "Fabrikam"}}),
        ])
    }

    #[test]
    fn repo_list_filters_sorts_and_limits_and_repo_get_lists_branches() {
        let (outcome, _) = ado(
            &["ado", "repo", "list", "WEB", "--limit", "1"],
            vec![page(vec![
                json!({"id": "r-2", "name": "web-e2e", "defaultBranch": "refs/heads/main", "isDisabled": false}),
                json!({"id": "r-1", "name": "web", "defaultBranch": "refs/heads/main", "size": 2048,
                    "webUrl": format!("{BASE}/Fabrikam/_git/web")}),
                json!({"id": "r-3", "name": "api"}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "web", "id": "r-1", "default_branch": "main", "size": 2048,
                "web_url": format!("{BASE}/Fabrikam/_git/web")}])
        );
        assert_eq!(
            outcome.stderr,
            "[1 of 2; --limit N, or narrow with PATTERN]\n"
        );

        let (outcome, transport) = ado(
            &["ado", "repo", "get", "web"],
            vec![
                Answer::json(
                    &json!({"id": "r-1", "name": "web", "project": {"name": "Fabrikam"},
                    "defaultBranch": "refs/heads/main", "remoteUrl": format!("{BASE}/Fabrikam/_git/web")}),
                ),
                page(vec![
                    json!({"name": "refs/heads/main"}),
                    json!({"name": "refs/heads/feature/x"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let repo = outcome.json();
        assert_eq!(repo["branches"], json!(["main", "feature/x"]));
        assert_eq!(repo["default_branch"], "main");
        assert_eq!(
            urls(&transport)[1],
            format!("{CODE}/git/repositories/r-1/refs?filter=heads/&$top=101&api-version=7.1")
        );
    }

    #[test]
    fn pr_list_filters_server_side_and_pages_until_enough_drafts_match() {
        let (outcome, transport) = ado(
            &[
                "ado", "pr", "list", "--author", "@me", "--target", "main", "--draft", "--limit",
                "1",
            ],
            vec![
                me(),
                page(vec![pr(1, false), pr(2, false)]),
                page(vec![pr(3, true)]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], 3);
        assert_eq!(rows[0]["is_draft"], true);
        assert_eq!(rows[0]["source"], "42-fix-login");
        assert_eq!(
            rows[0]["reviewers"],
            json!([{"name": "Sam Lee", "vote": "approved", "required": true},
                   {"name": "Web Team", "vote": "waiting"}])
        );
        assert_eq!(
            rows[0]["url"],
            format!("{BASE}/Fabrikam/_git/web/pullrequest/3")
        );
        let sent = urls(&transport);
        let criteria = "searchCriteria.status=active&searchCriteria.creatorId=u-1&searchCriteria.targetRefName=refs%2Fheads%2Fmain";
        assert_eq!(
            sent[1],
            format!("{CODE}/git/pullrequests?{criteria}&$top=2&$skip=0&api-version=7.1")
        );
        assert_eq!(
            sent[2],
            format!("{CODE}/git/pullrequests?{criteria}&$top=2&$skip=2&api-version=7.1")
        );

        let (outcome, transport) = ado(
            &[
                "ado", "pr", "list", "--repo", "web", "--status", "all", "--limit", "1",
            ],
            vec![page(vec![pr(1, false), pr(2, false)])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome.stderr.starts_with("[first 1; more match"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            urls(&transport),
            [format!(
                "{CODE}/git/repositories/web/pullrequests?searchCriteria.status=all&$top=2&$skip=0&api-version=7.1"
            )]
        );

        let (outcome, transport) = ado(
            &[
                "ado",
                "pr",
                "list",
                "--since",
                "2026-09-01",
                "--until",
                "2026-09-02",
            ],
            vec![page(vec![])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            urls(&transport)[0].contains(
                "&searchCriteria.minTime=2026-09-01T00%3A00%3A00Z&searchCriteria.maxTime=2026-09-02T00%3A00%3A00Z&"
            ),
            "{:?}",
            urls(&transport)
        );
    }

    #[test]
    fn pr_get_gathers_votes_work_items_policies_and_the_open_threads() {
        let (outcome, transport) = ado(
            &["ado", "pr", "get", "17", "--comments", "1"],
            vec![
                Answer::json(&pr(17, false)),
                page(vec![json!({"id": "42", "url": "x"})]),
                page(vec![
                    json!({"status": "approved", "configuration": {"type": {"displayName": "Minimum number of reviewers"}, "settings": {}}}),
                    json!({"status": "rejected", "configuration": {"type": {"displayName": "Build"},
                        "settings": {"displayName": "web-ci"}}, "context": {"buildId": 991}}),
                    json!({"status": "notApplicable", "configuration": {"type": {"displayName": "Comment requirements"}}}),
                ]),
                page(vec![
                    json!({"id": 1, "status": "active", "lastUpdatedDate": "2026-09-27T00:00:00Z",
                        "comments": [{"author": {"displayName": "Sam Lee"}, "content": "Older", "commentType": "text"}]}),
                    json!({"id": 2, "status": "active", "lastUpdatedDate": "2026-09-28T00:00:00Z",
                        "threadContext": {"filePath": "/src/login.ts", "rightFileStart": {"line": 12}},
                        "comments": [{"author": {"displayName": "Sam Lee"}, "content": "Null check?", "commentType": "text"},
                                     {"author": {"displayName": "Jane Doe"}, "content": "Done", "commentType": "text"}]}),
                    json!({"id": 3, "status": "fixed", "comments": [{"content": "Resolved", "commentType": "text"}]}),
                    json!({"id": 4, "status": "active", "comments": [{"content": "Jane voted 10", "commentType": "system"}]}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["description"], "Fixes **login**");
        assert_eq!(got["work_items"], json!([42]));
        assert_eq!(
            got["policies"],
            json!([{"name": "Minimum number of reviewers", "status": "approved"},
                   {"name": "web-ci", "status": "rejected", "run_id": 991}])
        );
        assert_eq!(got["open_threads"], 2);
        assert_eq!(
            got["threads"],
            json!([{"id": 2, "status": "active", "author": "Sam Lee", "file": "/src/login.ts", "line": 12,
                "date": "2026-09-28T00:00:00Z", "text": "Null check?", "replies": 1}])
        );
        assert_eq!(got["last_merge_source_commit"], "abc123");
        let sent = urls(&transport);
        assert_eq!(
            sent[0],
            format!("{CODE}/git/pullrequests/17?api-version=7.1")
        );
        assert_eq!(
            sent[2],
            format!(
                "{CODE}/policy/evaluations?artifactId=vstfs%3A%2F%2F%2FCodeReview%2FCodeReviewId%2Fp-1%2F17&api-version=7.1-preview.1"
            )
        );

        let (outcome, _) = ado(
            &["ado", "pr", "get", "9"],
            vec![Answer::status(
                404,
                r#"{"message":"TF401180: The requested pull request was not found."}"#,
            )],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }

    #[test]
    fn pr_create_opens_one_with_its_work_items_when_none_is_open() {
        let plans = dry_run(
            &[
                "ado",
                "pr",
                "create",
                "--repo",
                "web",
                "--source",
                "42-fix-login",
                "--title",
                "Fix login",
                "--workitem",
                "42",
                "--draft",
            ],
            vec![repos(), page(vec![])],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/git/repositories/r-1/pullrequests?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"sourceRefName": "refs/heads/42-fix-login", "targetRefName": "refs/heads/main",
                "title": "Fix login", "description": "", "isDraft": true, "workItemRefs": [{"id": "42"}]})
        );
    }

    #[test]
    fn pr_create_reuses_the_open_one_and_repairs_a_missing_link() {
        let work_item = json!({"id": 42, "rev": 5, "fields": {"System.Title": "Fix"}});
        let (outcome, transport) = ado(
            &[
                "ado",
                "pr",
                "create",
                "--repo",
                "web",
                "--source",
                "42-fix-login",
                "--title",
                "Fix login",
                "--workitem",
                "42",
            ],
            vec![
                repos(),
                page(vec![pr(17, false)]),
                page(vec![]),
                Answer::json(&work_item),
                Answer::json(&work_item),
                page(vec![json!({"id": "42"})]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], 17);
        assert_eq!(got["created"], false);
        assert_eq!(got["work_items"], json!([42]));
        let patch = &transport.sent()[4];
        assert_eq!(
            patch.url,
            format!("{BASE}/_apis/wit/workitems/42?api-version=7.1")
        );
        assert_eq!(
            patch.body.as_ref().unwrap()[1]["value"]["url"],
            "vstfs:///Git/PullRequestId/p-1%2Fr-1%2F17"
        );
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{CODE}/git/repositories/r-1/pullrequests?searchCriteria.status=active&searchCriteria.sourceRefName=refs%2Fheads%2F42-fix-login&searchCriteria.targetRefName=refs%2Fheads%2Fmain&api-version=7.1"
            )
        );

        let (outcome, _) = ado(
            &[
                "ado",
                "pr",
                "create",
                "--repo",
                "web",
                "--source",
                "42-fix-login",
                "--title",
                "Fix login",
                "--workitem",
                "42",
            ],
            vec![
                repos(),
                page(vec![pr(17, false)]),
                page(vec![]),
                Answer::json(&work_item),
                Answer::json(&work_item),
                page(vec![]),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(outcome.json()["id"], 17, "the pull request prints anyway");
        assert!(outcome.json().get("work_items").is_none());
        assert!(
            outcome
                .stderr
                .contains("pull request 17 is open, but work item 42 did not link"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("hint: run the same command again"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = ado(
            &[
                "ado", "pr", "create", "--repo", "web", "--source", "x", "--title", "Two",
            ],
            vec![repos(), page(vec![pr(17, false), pr(18, false)])],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(outcome.stderr.contains("17, 18"), "{}", outcome.stderr);
    }

    #[test]
    fn a_vote_is_put_under_your_own_id() {
        let plans = dry_run(
            &["ado", "pr", "vote", "17", "suggest"],
            vec![Answer::json(&pr(17, false)), me()],
        );
        assert_eq!(plans[0]["method"], "PUT");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/git/repositories/r-1/pullrequests/17/reviewers/u-1?api-version=7.1")
        );
        assert_eq!(plans[0]["body"], json!({"vote": 5}));

        let (outcome, _) = ado(
            &["ado", "pr", "vote", "17", "approve"],
            vec![
                Answer::json(&pr(17, false)),
                me(),
                Answer::json(&json!({"id": "u-1", "vote": 10})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!({"id": 17, "vote": "approved"}));
        let (outcome, _) = ado(&["ado", "pr", "vote", "17", "lgtm"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn update_sets_auto_complete_as_you_and_publishes_a_draft_in_one_patch() {
        let plans = dry_run(
            &[
                "ado",
                "pr",
                "update",
                "17",
                "--autocomplete",
                "on",
                "--draft",
                "false",
            ],
            vec![Answer::json(&pr(17, true)), me()],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/git/repositories/r-1/pullrequests/17?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"isDraft": false, "autoCompleteSetBy": {"id": "u-1"}})
        );

        let plans = dry_run(
            &["ado", "pr", "update", "17", "--autocomplete", "off"],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(
            plans[0]["body"],
            json!({"autoCompleteSetBy": {"id": "00000000-0000-0000-0000-000000000000"}})
        );

        let (outcome, transport) = ado(&["ado", "pr", "update", "17"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn link_writes_the_artifact_link_on_the_work_item_unless_it_is_there() {
        let work_item = json!({"id": 42, "rev": 5, "fields": {}});
        let plans = dry_run(
            &["ado", "pr", "link", "17", "--workitem", "42"],
            vec![Answer::json(&pr(17, false)), Answer::json(&work_item)],
        );
        assert_eq!(
            plans[0]["url"],
            format!("{BASE}/_apis/wit/workitems/42?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!([{"op": "test", "path": "/rev", "value": 5},
                {"op": "add", "path": "/relations/-", "value": {"rel": "ArtifactLink",
                    "url": "vstfs:///Git/PullRequestId/p-1%2Fr-1%2F17", "attributes": {"name": "Pull Request"}}}])
        );

        let linked = json!({"id": 42, "rev": 5, "relations": [
            {"rel": "ArtifactLink", "url": "vstfs:///Git/PullRequestId/p-1%2fr-1%2f17"}]});
        let (outcome, transport) = ado(
            &["ado", "pr", "link", "17", "--workitem", "42"],
            vec![Answer::json(&pr(17, false)), Answer::json(&linked)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"pr": 17, "work_item": 42, "already_linked": true})
        );
        assert!(transport.sent().iter().all(|sent| sent.method.is_read()));
    }

    #[test]
    fn a_comment_starts_a_thread_of_markdown() {
        let plans = dry_run(
            &["ado", "pr", "comment", "17", "LGTM, one nit"],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/git/repositories/r-1/pullRequests/17/threads?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"comments": [{"parentCommentId": 0, "content": "LGTM, one nit", "commentType": "text"}], "status": "active"})
        );
        let (outcome, _) = ado(
            &["ado", "pr", "comment", "17", "ok"],
            vec![
                Answer::json(&pr(17, false)),
                Answer::json(&json!({"id": 88, "status": "active"})),
            ],
        );
        assert_eq!(outcome.json(), json!({"pr": 17, "thread_id": 88}));
    }

    #[test]
    fn complete_takes_the_head_it_read_and_needs_yes_and_an_active_pr() {
        let plans = dry_run(
            &[
                "ado",
                "pr",
                "complete",
                "17",
                "--strategy",
                "rebase",
                "--keep-source",
            ],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["body"],
            json!({"status": "completed", "lastMergeSourceCommit": {"commitId": "abc123"},
                "completionOptions": {"mergeStrategy": "rebase", "deleteSourceBranch": false, "transitionWorkItems": true}})
        );

        let (outcome, transport) = ado(&["ado", "pr", "complete", "17"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("--yes"), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());

        let mut done = pr(17, false);
        done["status"] = json!("completed");
        let (outcome, _) = ado(
            &["ado", "pr", "complete", "17", "--yes"],
            vec![Answer::json(&done)],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome.stderr.contains("already completed"),
            "{}",
            outcome.stderr
        );

        let mut merged = pr(17, false);
        merged["status"] = json!("completed");
        let (outcome, _) = ado(
            &["ado", "pr", "complete", "17", "--yes"],
            vec![Answer::json(&pr(17, false)), Answer::json(&merged)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["status"], "completed");
    }

    #[test]
    fn abandon_is_one_patch_after_the_status_check() {
        let plans = dry_run(
            &["ado", "pr", "abandon", "17"],
            vec![Answer::json(&pr(17, false))],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(plans[0]["body"], json!({"status": "abandoned"}));
    }
}
