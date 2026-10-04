//! A synthetic registry shaped like the planned domains: real clap args, real
//! return structs, handlers that go through `ctx.read`/`ctx.write`. The names
//! are made up; only the shapes matter.

#![allow(dead_code)]

use std::process::Command as Process;

use agent_cli_core::{Check, Command, Config, Ctx, Domain, Effect, Method, Op, Request, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

// ---------- tracker: tickets, pull requests, builds, approvals ----------

#[derive(clap::Args)]
pub struct TicketList {
    /// name, email or @me
    #[arg(long)]
    assignee: Option<String>,
    /// e.g. Active, "In Progress"
    #[arg(long)]
    state: Vec<String>,
    /// words in the title
    #[arg(long)]
    text: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Serialize, JsonSchema)]
pub struct TicketRow {
    id: u64,
    title: String,
    state: String,
    assignee: Option<String>,
    tags: Vec<String>,
}

fn ticket_list(ctx: &Ctx, args: TicketList) -> Result<Vec<TicketRow>> {
    let _ = ctx.globals();
    Ok((1..=args.limit as u64)
        .map(|id| TicketRow {
            id,
            title: format!("ticket {id}: {}", args.text.clone().unwrap_or_default()),
            state: args
                .state
                .first()
                .cloned()
                .unwrap_or_else(|| "Active".to_owned()),
            assignee: args.assignee.clone(),
            tags: Vec::new(),
        })
        .collect())
}

command! {
    pub TICKET_LIST = ["tracker", "ticket", "list"], Read,
    "List tickets matching filters",
    keywords: ["backlog", "assigned", "open", "mine"],
    example: "tracker ticket list --assignee @me --state Active --fields id,title,state",
    run: ticket_list,
}

#[derive(clap::Args)]
pub struct TicketId {
    /// the ticket number
    id: u64,
}

#[derive(Serialize, JsonSchema)]
pub struct Ticket {
    id: u64,
    title: String,
    state: String,
    description: String,
    rev: u32,
}

fn ticket_get(ctx: &Ctx, args: TicketId) -> Result<Ticket> {
    let body = ctx
        .read(Request::get(format!(
            "https://tracker.example/tickets/{}",
            args.id
        )))?
        .json()?;
    let text = |key: &str| body[key].as_str().unwrap_or_default().to_owned();
    Ok(Ticket {
        id: args.id,
        title: text("title"),
        state: text("state"),
        description: text("description"),
        rev: body["rev"].as_u64().map_or(1, |rev| rev as u32),
    })
}

command! {
    pub TICKET_GET = ["tracker", "ticket", "get"], Read,
    "Show one ticket with its description and revision",
    keywords: ["bug", "details", "describe"],
    example: "tracker ticket get 42",
    run: ticket_get,
}

#[derive(clap::Args)]
pub struct TicketCreate {
    /// the ticket's title
    #[arg(long)]
    title: String,
    /// Bug, Task, Story
    #[arg(long, default_value = "Task")]
    r#type: String,
}

#[derive(Serialize, JsonSchema)]
pub struct Created {
    id: u64,
    url: String,
}

/// Reads the project first, then creates: under --dry-run the read runs and
/// the create is only planned.
fn ticket_create(ctx: &Ctx, args: TicketCreate) -> Result<Created> {
    let project = ctx
        .read(Request::get("https://tracker.example/project"))?
        .json()?;
    let created = ctx.write(
        Effect::Write,
        Request::new(Method::Post, "https://tracker.example/tickets")
            .header("Authorization", "Bearer do-not-print-me")
            .json(json!({"title": args.title, "type": args.r#type, "project": project["id"]})),
    )?;
    let body = created.json()?;
    Ok(Created {
        id: body["id"].as_u64().unwrap_or_default(),
        url: body["url"].as_str().unwrap_or_default().to_owned(),
    })
}

command! {
    pub TICKET_CREATE = ["tracker", "ticket", "create"], Write,
    "Create a ticket",
    keywords: ["new", "file", "open", "report"],
    example: "tracker ticket create --title 'Login fails on Safari' --type Bug",
    run: ticket_create,
}

#[derive(clap::Args)]
pub struct TicketUpdate {
    id: u64,
    #[arg(long)]
    state: Option<String>,
    #[arg(long)]
    title: Option<String>,
    /// fail with exit 5 unless the ticket is at this revision
    #[arg(long)]
    if_rev: Option<u32>,
}

fn ticket_update(ctx: &Ctx, args: TicketUpdate) -> Result<Created> {
    ctx.write(
        Effect::Write,
        Request::new(
            Method::Patch,
            format!("https://tracker.example/tickets/{}", args.id),
        )
        .json(json!({"state": args.state, "title": args.title, "rev": args.if_rev})),
    )?;
    Ok(Created {
        id: args.id,
        url: String::new(),
    })
}

command! {
    pub TICKET_UPDATE = ["tracker", "ticket", "update"], Write,
    "Change a ticket's state, title or fields",
    keywords: ["edit", "close", "resolve", "move"],
    example: "tracker ticket update 42 --state Resolved",
    run: ticket_update,
}

#[derive(clap::Args)]
pub struct Comment {
    id: u64,
    /// the comment text
    text: String,
}

fn ticket_comment(ctx: &Ctx, args: Comment) -> Result<Created> {
    ctx.write(
        Effect::Write,
        Request::new(
            Method::Post,
            format!("https://tracker.example/tickets/{}/comments", args.id),
        )
        .json(json!({"text": args.text})),
    )?;
    Ok(Created {
        id: args.id,
        url: String::new(),
    })
}

command! {
    pub TICKET_COMMENT = ["tracker", "ticket", "comment"], Write,
    "Add a comment to a ticket",
    keywords: ["note", "reply", "discussion"],
    example: "tracker ticket comment 42 'Fixed in the next build'",
    run: ticket_comment,
}

fn ticket_delete(ctx: &Ctx, args: TicketId) -> Result<Created> {
    ctx.write(
        Effect::Destructive,
        Request::new(
            Method::Delete,
            format!("https://tracker.example/tickets/{}", args.id),
        ),
    )?;
    Ok(Created {
        id: args.id,
        url: String::new(),
    })
}

command! {
    pub TICKET_DELETE = ["tracker", "ticket", "delete"], Destructive,
    "Delete a ticket for good",
    keywords: ["remove", "destroy"],
    example: "tracker ticket delete 42 --yes",
    run: ticket_delete,
}

#[derive(clap::Args)]
pub struct PrList {
    /// active, completed, abandoned
    #[arg(long, value_parser = ["active", "completed", "abandoned"])]
    status: Option<String>,
    #[arg(long)]
    repo: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Serialize, JsonSchema)]
pub struct Reviewer {
    name: String,
    vote: i32,
}

#[derive(Serialize, JsonSchema)]
pub struct Pr {
    id: u64,
    title: String,
    status: String,
    source: String,
    target: String,
    reviewers: Vec<Reviewer>,
}

fn pr_list(_: &Ctx, args: PrList) -> Result<Vec<Pr>> {
    Ok((1..=args.limit.min(3) as u64)
        .map(|id| Pr {
            id,
            title: format!("change {id}"),
            status: args.status.clone().unwrap_or_else(|| "active".to_owned()),
            source: "feature".to_owned(),
            target: "main".to_owned(),
            reviewers: vec![Reviewer {
                name: "kim".to_owned(),
                vote: 10,
            }],
        })
        .collect())
}

command! {
    pub PR_LIST = ["tracker", "pr", "list"], Read,
    "List pull requests across repositories",
    keywords: ["review", "open", "merge", "requests"],
    example: "tracker pr list --status active --fields id,title,reviewers.name",
    run: pr_list,
}

#[derive(clap::Args)]
pub struct PrId {
    id: u64,
}

fn pr_get(ctx: &Ctx, args: PrId) -> Result<Pr> {
    let mut list = pr_list(
        ctx,
        PrList {
            status: None,
            repo: None,
            limit: 1,
        },
    )?;
    let mut pr = list.remove(0);
    pr.id = args.id;
    Ok(pr)
}

command! {
    pub PR_GET = ["tracker", "pr", "get"], Read,
    "Show a pull request with its reviewers and votes",
    keywords: ["review", "approved", "details"],
    example: "tracker pr get 7",
    run: pr_get,
}

#[derive(clap::Args)]
pub struct PrCreate {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    source: String,
    #[arg(long, default_value = "main")]
    target: String,
    #[arg(long)]
    title: Option<String>,
}

fn pr_create(ctx: &Ctx, args: PrCreate) -> Result<Created> {
    ctx.write(
        Effect::Write,
        Request::new(
            Method::Post,
            format!("https://tracker.example/repos/{}/prs", args.repo),
        )
        .json(json!({"source": args.source, "target": args.target, "title": args.title})),
    )?;
    Ok(Created {
        id: 1,
        url: String::new(),
    })
}

command! {
    pub PR_CREATE = ["tracker", "pr", "create"], Write,
    "Open a pull request from a branch",
    keywords: ["new", "raise", "submit"],
    example: "tracker pr create --repo web --source feature/login",
    run: pr_create,
}

#[derive(clap::Args)]
pub struct Vote {
    id: u64,
    /// approve, reject, wait, reset
    #[arg(value_parser = ["approve", "reject", "wait", "reset"])]
    vote: String,
}

fn pr_vote(ctx: &Ctx, args: Vote) -> Result<Created> {
    ctx.write(
        Effect::Write,
        Request::new(
            Method::Put,
            format!("https://tracker.example/prs/{}/vote", args.id),
        )
        .json(json!({"vote": args.vote})),
    )?;
    Ok(Created {
        id: args.id,
        url: String::new(),
    })
}

command! {
    pub PR_VOTE = ["tracker", "pr", "vote"], Write,
    "Vote on a pull request as a reviewer",
    keywords: ["review", "sign off", "lgtm"],
    example: "tracker pr vote 7 approve",
    run: pr_vote,
}

fn pr_complete(ctx: &Ctx, args: PrId) -> Result<Created> {
    ctx.write(
        Effect::Destructive,
        Request::new(
            Method::Patch,
            format!("https://tracker.example/prs/{}", args.id),
        )
        .json(json!({"status": "completed"})),
    )?;
    Ok(Created {
        id: args.id,
        url: String::new(),
    })
}

command! {
    pub PR_COMPLETE = ["tracker", "pr", "complete"], Destructive,
    "Complete a pull request, merging it into its target",
    keywords: ["merge", "squash", "finish"],
    example: "tracker pr complete 7 --yes",
    run: pr_complete,
}

fn pr_abandon(ctx: &Ctx, args: PrId) -> Result<Created> {
    ctx.write(
        Effect::Destructive,
        Request::new(
            Method::Patch,
            format!("https://tracker.example/prs/{}", args.id),
        )
        .json(json!({"status": "abandoned"})),
    )?;
    Ok(Created {
        id: args.id,
        url: String::new(),
    })
}

command! {
    pub PR_ABANDON = ["tracker", "pr", "abandon"], Destructive,
    "Abandon a pull request without merging",
    keywords: ["close", "drop", "discard"],
    example: "tracker pr abandon 7 --yes",
    run: pr_abandon,
}

#[derive(clap::Args)]
pub struct BuildList {
    #[arg(long)]
    pipeline: Option<String>,
    #[arg(long)]
    branch: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Serialize, JsonSchema)]
pub struct BuildRow {
    id: u64,
    pipeline: String,
    branch: String,
    status: String,
    result: Option<String>,
    started: String,
}

fn build_list(_: &Ctx, args: BuildList) -> Result<Vec<BuildRow>> {
    Ok((1..=args.limit.min(2) as u64)
        .map(|id| BuildRow {
            id,
            pipeline: args.pipeline.clone().unwrap_or_else(|| "ci".to_owned()),
            branch: args.branch.clone().unwrap_or_else(|| "main".to_owned()),
            status: "completed".to_owned(),
            result: Some("succeeded".to_owned()),
            started: "2026-09-29T10:00:00Z".to_owned(),
        })
        .collect())
}

command! {
    pub BUILD_LIST = ["tracker", "build", "list"], Read,
    "List recent builds with their status and result",
    keywords: ["runs", "history", "failed"],
    example: "tracker build list --branch main --fields id,status,result",
    run: build_list,
}

#[derive(clap::Args)]
pub struct BuildId {
    id: u64,
}

fn build_get(ctx: &Ctx, args: BuildId) -> Result<BuildRow> {
    let mut rows = build_list(
        ctx,
        BuildList {
            pipeline: None,
            branch: None,
            limit: 1,
        },
    )?;
    let mut row = rows.remove(0);
    row.id = args.id;
    Ok(row)
}

command! {
    pub BUILD_GET = ["tracker", "build", "get"], Read,
    "Show one build's status, result and timing",
    keywords: ["details", "state"],
    example: "tracker build get 311",
    run: build_get,
}

#[derive(clap::Args)]
pub struct BuildLogs {
    id: u64,
    /// how many lines from the end
    #[arg(long, default_value_t = 200)]
    tail: usize,
}

#[derive(Serialize, JsonSchema)]
pub struct Text {
    text: String,
    lines: usize,
}

fn build_logs(_: &Ctx, args: BuildLogs) -> Result<Text> {
    let text: String = (0..args.tail)
        .map(|i| format!("step {i}: compiling module {i} of the build\n"))
        .collect();
    Ok(Text {
        text,
        lines: args.tail,
    })
}

command! {
    pub BUILD_LOGS = ["tracker", "build", "logs"], Read,
    "Show the log output of a build",
    keywords: ["output", "console", "why", "failed"],
    example: "tracker build logs 311 --tail 100",
    run: build_logs,
}

#[derive(clap::Args)]
pub struct BuildRun {
    #[arg(long)]
    pipeline: String,
    #[arg(long)]
    branch: String,
}

fn build_run(ctx: &Ctx, args: BuildRun) -> Result<Created> {
    ctx.write(
        Effect::Write,
        Request::new(
            Method::Post,
            format!("https://tracker.example/pipelines/{}/runs", args.pipeline),
        )
        .json(json!({"branch": args.branch})),
    )?;
    Ok(Created {
        id: 1,
        url: String::new(),
    })
}

command! {
    pub BUILD_RUN = ["tracker", "build", "run"], Write,
    "Queue a new build of a pipeline for a branch",
    keywords: ["trigger", "start", "queue", "kick off"],
    example: "tracker build run --pipeline ci --branch main",
    run: build_run,
}

fn build_cancel(ctx: &Ctx, args: BuildId) -> Result<Created> {
    ctx.write(
        Effect::Destructive,
        Request::new(
            Method::Patch,
            format!("https://tracker.example/builds/{}", args.id),
        )
        .json(json!({"status": "cancelling"})),
    )?;
    Ok(Created {
        id: args.id,
        url: String::new(),
    })
}

command! {
    pub BUILD_CANCEL = ["tracker", "build", "cancel"], Destructive,
    "Cancel a build that is queued or running",
    keywords: ["stop", "abort", "kill"],
    example: "tracker build cancel 311 --yes",
    run: build_cancel,
}

#[derive(clap::Args)]
pub struct NoArgs {}

#[derive(Serialize, JsonSchema)]
pub struct Approval {
    id: u64,
    stage: String,
    pipeline: String,
    requested: String,
}

fn approval_list(_: &Ctx, _: NoArgs) -> Result<Vec<Approval>> {
    Ok(vec![Approval {
        id: 5,
        stage: "production".into(),
        pipeline: "release".into(),
        requested: "2026-09-29".into(),
    }])
}

command! {
    pub APPROVAL_LIST = ["tracker", "approval", "list"], Read,
    "List deployment approvals waiting on you",
    keywords: ["pending", "gate", "waiting"],
    example: "tracker approval list --fields id,stage,pipeline",
    run: approval_list,
}

#[derive(clap::Args)]
pub struct ApprovalId {
    id: u64,
    #[arg(long)]
    comment: Option<String>,
}

fn approval_approve(ctx: &Ctx, args: ApprovalId) -> Result<Created> {
    ctx.write(
        Effect::Destructive,
        Request::new(
            Method::Patch,
            format!("https://tracker.example/approvals/{}", args.id),
        )
        .json(json!({"status": "approved", "comment": args.comment})),
    )?;
    Ok(Created {
        id: args.id,
        url: String::new(),
    })
}

command! {
    pub APPROVAL_APPROVE = ["tracker", "approval", "approve"], Destructive,
    "Approve a pending deployment gate",
    keywords: ["release", "deploy", "production"],
    example: "tracker approval approve 5 --yes",
    run: approval_approve,
}

pub const TRACKER: Domain = Domain {
    name: "tracker",
    summary: "Tracker",
    commands: &[
        TICKET_LIST,
        TICKET_GET,
        TICKET_CREATE,
        TICKET_UPDATE,
        TICKET_COMMENT,
        TICKET_DELETE,
        PR_LIST,
        PR_GET,
        PR_CREATE,
        PR_VOTE,
        PR_COMPLETE,
        PR_ABANDON,
        BUILD_LIST,
        BUILD_GET,
        BUILD_LOGS,
        BUILD_RUN,
        BUILD_CANCEL,
        APPROVAL_LIST,
        APPROVAL_APPROVE,
    ],
    synonyms: &[
        ("bug", &["ticket"]),
        ("issue", &["ticket"]),
        ("story", &["ticket"]),
        ("task", &["ticket"]),
        ("pull request", &["pr"]),
        ("ci", &["build"]),
        ("pipeline", &["build"]),
    ],
    status: |config| {
        if config.has_section("tracker") {
            "tracker \u{2713}".to_owned()
        } else {
            "tracker not set up".to_owned()
        }
    },
    doctor: |ctx| {
        let org: Result<TrackerConfig> = ctx.section("tracker");
        match org {
            Ok(config) if !config.org.is_empty() => vec![Check::ok("org", config.org)],
            Ok(_) => vec![Check::failed(
                "org",
                "no org configured",
                "set [tracker] org in the config file",
            )],
            Err(error) => vec![Check::failed(
                "config",
                format!("{error:#}"),
                "fix [tracker]",
            )],
        }
    },
};

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub struct TrackerConfig {
    pub org: String,
}

// ---------- vault: secrets ----------

#[derive(clap::Args)]
pub struct SecretList {
    /// words in the secret's name
    query: Option<String>,
    #[arg(long)]
    vault: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Serialize, JsonSchema)]
pub struct SecretRow {
    vault: String,
    name: String,
    enabled: bool,
    expires: Option<String>,
}

fn secret_list(_: &Ctx, args: SecretList) -> Result<Vec<SecretRow>> {
    Ok(vec![SecretRow {
        vault: args.vault.unwrap_or_else(|| "kv-main".into()),
        name: args.query.unwrap_or_else(|| "db-password".into()),
        enabled: true,
        expires: None,
    }])
}

command! {
    pub SECRET_LIST = ["vault", "secret", "list"], Read,
    "List secret names and metadata, never values",
    keywords: ["keyvault", "names", "expiry"],
    example: "vault secret list db --fields vault,name,expires",
    run: secret_list,
}

#[derive(clap::Args)]
pub struct SecretGet {
    name: String,
    #[arg(long)]
    vault: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct SecretValue {
    name: String,
    version: String,
    value: String,
}

fn secret_get(ctx: &Ctx, args: SecretGet) -> Result<SecretValue> {
    let answer = ctx
        .read(Request::get(format!(
            "https://kv.example/secrets/{}",
            args.name
        )))?
        .json()?;
    let value = agent_cli_core::Secret::new(answer["value"].as_str().unwrap_or_default());
    Ok(SecretValue {
        name: args.name,
        version: "v1".into(),
        value: value.expose().to_owned(),
    })
}

command! {
    pub SECRET_GET = ["vault", "secret", "get"], Reveal,
    "Read a secret's value",
    keywords: ["reveal", "value", "show"],
    example: "vault secret get db-password --output /tmp/db-password --fields value",
    run: secret_get,
}

#[derive(Serialize, JsonSchema)]
pub struct KeyRow {
    name: String,
    kind: String,
}

fn key_list(_: &Ctx, _: NoArgs) -> Result<Vec<KeyRow>> {
    Ok(vec![KeyRow {
        name: "signing".into(),
        kind: "RSA".into(),
    }])
}

command! {
    pub KEY_LIST = ["vault", "key", "list"], Read,
    "List cryptographic keys in the vaults",
    keywords: ["rsa", "signing", "encryption"],
    example: "vault key list --fields name,kind",
    run: key_list,
}

pub const VAULT: Domain = Domain {
    name: "vault",
    summary: "Vault",
    commands: &[SECRET_LIST, SECRET_GET, KEY_LIST],
    synonyms: &[("password", &["secret"]), ("credential", &["secret"])],
    status: |_| String::new(),
    doctor: |_| Vec::new(),
};

// ---------- db: SQL ----------

#[derive(clap::Args)]
pub struct QueryRun {
    /// the connection name from the config
    #[arg(long)]
    conn: String,
    /// the SQL text
    #[arg(allow_hyphen_values = true)]
    sql: String,
}

/// A SQL batch as an op, the way the sql domain will implement one.
pub struct Batch {
    pub sql: String,
}

impl Op for Batch {
    type Output = u64;

    fn plan(&self) -> Value {
        json!({"sql": self.sql})
    }

    fn writes(&self) -> bool {
        !self.sql.trim_start().to_lowercase().starts_with("select")
    }

    fn perform(self, _: &Ctx) -> Result<u64> {
        Ok(3)
    }
}

#[derive(Serialize, JsonSchema)]
pub struct QueryResult {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    rows_affected: u64,
}

fn query_run(ctx: &Ctx, args: QueryRun) -> Result<QueryResult> {
    let batch = Batch { sql: args.sql };
    let rows_affected = if batch.writes() {
        ctx.write(Effect::Destructive, batch)?
    } else {
        ctx.read(batch)?
    };
    Ok(QueryResult {
        columns: vec!["n".into()],
        rows: vec![vec![json!(1)]],
        rows_affected,
    })
}

command! {
    pub QUERY_RUN = ["db", "query", "run"], Varies,
    "Run SQL against a named connection",
    keywords: ["select", "statement", "execute", "database"],
    example: "db query run --conn local 'select 1'",
    run: query_run,
}

#[derive(clap::Args)]
pub struct TableList {
    #[arg(long)]
    conn: String,
    /// a LIKE pattern over schema.name
    pattern: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Serialize, JsonSchema)]
pub struct TableRow {
    schema: String,
    name: String,
    kind: String,
}

fn table_list(_: &Ctx, _: TableList) -> Result<Vec<TableRow>> {
    Ok(vec![TableRow {
        schema: "dbo".into(),
        name: "orders".into(),
        kind: "table".into(),
    }])
}

command! {
    pub TABLE_LIST = ["db", "table", "list"], Read,
    "List tables and views in a database",
    keywords: ["schema", "objects", "catalog"],
    example: "db table list --conn local 'dbo.%' --fields schema,name",
    run: table_list,
}

#[derive(clap::Args)]
pub struct TableGet {
    #[arg(long)]
    conn: String,
    /// schema.name
    name: String,
}

#[derive(Serialize, JsonSchema)]
pub struct Column {
    name: String,
    r#type: String,
    nullable: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct TableDetail {
    schema: String,
    name: String,
    columns: Vec<Column>,
    ddl: String,
}

fn table_get(_: &Ctx, args: TableGet) -> Result<TableDetail> {
    Ok(TableDetail {
        schema: "dbo".into(),
        name: args.name,
        columns: Vec::new(),
        ddl: String::new(),
    })
}

command! {
    pub TABLE_GET = ["db", "table", "get"], Read,
    "Show a table's columns and a DDL sketch",
    keywords: ["describe", "structure", "fields"],
    example: "db table get --conn local dbo.orders",
    run: table_get,
}

#[derive(Serialize, JsonSchema)]
pub struct Connection {
    name: String,
    kind: String,
    host: String,
    read_only: bool,
}

fn connection_list(_: &Ctx, _: NoArgs) -> Result<Vec<Connection>> {
    Ok(vec![Connection {
        name: "local".into(),
        kind: "mssql".into(),
        host: "localhost".into(),
        read_only: true,
    }])
}

command! {
    pub CONNECTION_LIST = ["db", "connection", "list"], Read,
    "List configured database connections",
    keywords: ["servers", "databases", "config"],
    example: "db connection list --fields name,kind,host",
    run: connection_list,
}

pub const DB: Domain = Domain {
    name: "db",
    summary: "SQL",
    commands: &[QUERY_RUN, TABLE_LIST, TABLE_GET, CONNECTION_LIST],
    synonyms: &[("sql", &["query"]), ("view", &["table"])],
    status: |_| "db 1 connection".to_owned(),
    doctor: |_| {
        vec![Check::failed(
            "local",
            "cannot reach localhost:1433",
            "start the database",
        )]
    },
};

// ---------- cluster: pods and deployments ----------

#[derive(clap::Args)]
pub struct Scope {
    #[arg(long)]
    namespace: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Serialize, JsonSchema)]
pub struct Pod {
    name: String,
    status: String,
    ready: String,
    restarts: u32,
    node: String,
}

fn pod_list(_: &Ctx, _: Scope) -> Result<Vec<Pod>> {
    Ok(vec![Pod {
        name: "api-1".into(),
        status: "Running".into(),
        ready: "1/1".into(),
        restarts: 0,
        node: "n1".into(),
    }])
}

command! {
    pub POD_LIST = ["cluster", "pod", "list"], Read,
    "List pods in a namespace with status and restarts",
    keywords: ["kubectl", "workloads", "running"],
    example: "cluster pod list --namespace web --fields name,status,restarts",
    run: pod_list,
}

#[derive(clap::Args)]
pub struct PodName {
    name: String,
    #[arg(long)]
    namespace: Option<String>,
}

fn pod_get(_: &Ctx, args: PodName) -> Result<Pod> {
    Ok(Pod {
        name: args.name,
        status: "Running".into(),
        ready: "1/1".into(),
        restarts: 0,
        node: "n1".into(),
    })
}

command! {
    pub POD_GET = ["cluster", "pod", "get"], Read,
    "Show a pod's containers, events and owner",
    keywords: ["describe", "yaml", "details"],
    example: "cluster pod get api-1 --namespace web",
    run: pod_get,
}

#[derive(clap::Args)]
pub struct PodLogs {
    name: String,
    #[arg(long)]
    namespace: Option<String>,
    /// how many lines from the end
    #[arg(long, default_value_t = 2000)]
    tail: usize,
}

fn pod_logs(_: &Ctx, args: PodLogs) -> Result<Text> {
    let text: String = (0..args.tail)
        .map(|i| format!("{} request {i} served in 3ms\n", args.name))
        .collect();
    Ok(Text {
        text,
        lines: args.tail,
    })
}

command! {
    pub POD_LOGS = ["cluster", "pod", "logs"], Read,
    "Show recent log lines from a pod's container",
    keywords: ["output", "stdout", "crash"],
    example: "cluster pod logs api-1 --namespace web",
    run: pod_logs,
}

fn pod_delete(ctx: &Ctx, args: PodName) -> Result<Value> {
    let mut kubectl = Process::new("echo");
    kubectl.args(["pod", &args.name, "deleted"]);
    let output = ctx.write(Effect::Destructive, kubectl)?;
    Ok(json!({"deleted": args.name, "said": output.stdout.trim()}))
}

command! {
    pub POD_DELETE = ["cluster", "pod", "delete"], Destructive,
    "Delete a pod so its controller replaces it",
    keywords: ["kill", "remove", "evict"],
    example: "cluster pod delete api-1 --namespace web --yes",
    run: pod_delete,
}

#[derive(clap::Args)]
pub struct DeploymentName {
    name: String,
    #[arg(long)]
    namespace: Option<String>,
}

fn deployment_restart(ctx: &Ctx, args: DeploymentName) -> Result<Value> {
    let mut kubectl = Process::new("echo");
    kubectl.args(["rollout", "restart", &args.name]);
    ctx.write(Effect::Destructive, kubectl)?;
    Ok(json!({"restarted": args.name}))
}

command! {
    pub DEPLOYMENT_RESTART = ["cluster", "deployment", "restart"], Destructive,
    "Restart a deployment's pods with a rolling update",
    keywords: ["rollout", "bounce", "redeploy"],
    example: "cluster deployment restart api --namespace web --yes",
    run: deployment_restart,
}

#[derive(clap::Args)]
pub struct Scale {
    name: String,
    #[arg(long)]
    replicas: u32,
    #[arg(long)]
    namespace: Option<String>,
}

fn deployment_scale(ctx: &Ctx, args: Scale) -> Result<Value> {
    let mut kubectl = Process::new("echo");
    kubectl.args(["scale", &args.name, &args.replicas.to_string()]);
    ctx.write(Effect::Destructive, kubectl)?;
    Ok(json!({"scaled": args.name, "replicas": args.replicas}))
}

command! {
    pub DEPLOYMENT_SCALE = ["cluster", "deployment", "scale"], Destructive,
    "Set the number of replicas of a deployment",
    keywords: ["replicas", "instances", "resize"],
    example: "cluster deployment scale api --replicas 3 --namespace web --yes",
    run: deployment_scale,
}

pub const CLUSTER: Domain = Domain {
    name: "cluster",
    summary: "Kubernetes",
    commands: &[
        POD_LIST,
        POD_GET,
        POD_LOGS,
        POD_DELETE,
        DEPLOYMENT_RESTART,
        DEPLOYMENT_SCALE,
    ],
    synonyms: &[
        ("container", &["pod"]),
        ("k8s", &["cluster"]),
        ("kubernetes", &["cluster"]),
    ],
    status: |_: &Config| String::new(),
    doctor: |_| Vec::new(),
};

pub const DOMAINS: &[Domain] = &[TRACKER, VAULT, DB, CLUSTER];

/// The commands whose shapes the 1,000-command registry reuses.
pub fn templates() -> Vec<Command> {
    DOMAINS
        .iter()
        .flat_map(|domain| domain.commands.iter().copied())
        .collect()
}

/// Labeled queries for the synthetic registry, written as tasks.
pub const LABELED: &str = r#"
[[query]]
text = "list my open tickets"
expect = "tracker ticket list"
[[query]]
text = "tickets assigned to me"
expect = "tracker ticket list"
[[query]]
text = "show bug 42"
expect = "tracker ticket get"
[[query]]
text = "file a new bug"
expect = "tracker ticket create"
[[query]]
text = "change the state of a ticket"
expect = "tracker ticket update"
[[query]]
text = "add a comment to an issue"
expect = "tracker ticket comment"
[[query]]
text = "delete a ticket"
expect = "tracker ticket delete"
[[query]]
text = "open pull requests"
expect = "tracker pr list"
[[query]]
text = "who reviewed pull request 7"
expect = "tracker pr get"
[[query]]
text = "create a pull request"
expect = "tracker pr create"
[[query]]
text = "vote on a pull request"
expect = "tracker pr vote"
[[query]]
text = "merge a pull request"
expect = "tracker pr complete"
[[query]]
text = "abandon pr"
expect = "tracker pr abandon"
[[query]]
text = "recent ci builds"
expect = "tracker build list"
[[query]]
text = "get build status"
expect = "tracker build get"
[[query]]
text = "build logs"
expect = "tracker build logs"
[[query]]
text = "trigger a pipeline"
expect = "tracker build run"
[[query]]
text = "cancel a running build"
expect = "tracker build cancel"
[[query]]
text = "pending approvals"
expect = "tracker approval list"
[[query]]
text = "approve a production deployment"
expect = "tracker approval approve"
[[query]]
text = "list secrets in a vault"
expect = "vault secret list"
[[query]]
text = "read a password"
expect = "vault secret get"
[[query]]
text = "show secret value"
expect = "vault secret get"
[[query]]
text = "list signing keys"
expect = "vault key list"
[[query]]
text = "run a sql query"
expect = "db query run"
[[query]]
text = "execute a select statement"
expect = "db query run"
[[query]]
text = "list tables"
expect = "db table list"
[[query]]
text = "columns of a table"
expect = "db table get"
[[query]]
text = "database connections"
expect = "db connection list"
[[query]]
text = "list pods"
expect = "cluster pod list"
[[query]]
text = "container logs"
expect = "cluster pod logs"
[[query]]
text = "describe a pod"
expect = "cluster pod get"
[[query]]
text = "delete a pod"
expect = "cluster pod delete"
[[query]]
text = "restart a deployment"
expect = "cluster deployment restart"
[[query]]
text = "scale replicas"
expect = "cluster deployment scale"
"#;
