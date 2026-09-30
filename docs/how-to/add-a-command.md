# How to add a command

This guide adds a command to an existing domain, with everything the registry
checks demand: typed args and return value, a handler, the registration,
labeled search queries, fixture and dry-run tests, a recording in the contoso
world and the regenerated reference. For a new service, first follow
[How to add a domain](add-a-domain.md).

The worked example is the approvals resource of the ado domain, as it is in
`crates/ado/src/pipeline.rs`: `ado approval list`, a read that lists pending
deployment gates, and `ado approval approve`, a destructive change that takes
the `id` the list prints. Every snippet is the code in the repository, so its
tests keep this guide honest.

Prerequisites:

- A clone of this repository, Rust 1.88 or later and a C compiler.
- The service's API documentation, and a real answer (or its documented
  shape) for the fixture test.
- The fixtures build, for trying the command against the world:
  `cargo build --features fixtures -p agent-cli`, then
  `eval "$(scripts/trial-env.sh)"`.

## 1. Choose the path and the effect

A command is `agent-cli <domain> <resource> <verb>`. Before you write one, ask
search whether a command already does the job:

```sh
agent-cli search "which deployments are waiting for approval"
```

Take the verb from `VERBS` in `crates/core/src/registry.rs`: `list get create
update delete run wait cancel retry logs vote complete abandon link comment
approve reject connect restart scale bench`. Add a verb there only when none
fits; it is a deliberate one-line edit.

Pick the effect from what the command can do, not from what it usually does:

| Effect | Use it for |
|---|---|
| `Read` | Reads only, including a `POST` that only queries |
| `Write` | A change that is easy to undo |
| `Destructive` | A change that is hard to undo: delete, complete, approve, restart, scale. Core asks for `--yes` |
| `Reveal` | Prints a secret value. Core asks for `--reveal` or `--output FILE` |
| `Varies` | The input decides, like SQL. The handler sends each write through `ctx.write` |

`ado approval list` is a `Read`. `ado approval approve` is `Destructive`,
because an approved gate can deploy to production.

## 2. Write the args struct

Put the command in the domain crate's module for its resource; a new resource
can go in an existing module or a new one (with its `mod` line in the crate's
`lib.rs`). The args are a `clap::Args` struct, and the first line of each
field's doc comment is its help text. A list takes `--limit` with a default of
50:

```rust
#[derive(clap::Args)]
pub struct ApprovalListArgs {
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}
```

A command about one object takes its identifier as a positional, in the form
the sibling list prints as `id`:

```rust
#[derive(clap::Args)]
pub struct AnswerArgs {
    /// The approval's id, from approval list
    id: String,
    /// Why, for the record
    #[arg(long)]
    comment: Option<String>,
}
```

A flag name that already exists in another command must take the same kind of
value (`check_registry` compares them). See [Conventions](#conventions) for
the names time windows, limits and scopes must use.

## 3. Write the return struct

The return type derives `Serialize` and `JsonSchema`. Help renders it as the
`Returns:` line and search indexes its field names, so it must be exactly what
the command returns. Put the fields in the order an agent wants them: `id`
first, then names, then state. `None` fields and empty lists are dropped from
the output.

```rust
#[derive(Debug, Serialize, JsonSchema)]
pub struct ApprovalRow {
    /// What approval approve and reject take.
    id: String,
    pipeline: Option<String>,
    run_id: Option<i64>,
    run: Option<String>,
    instructions: Option<String>,
    created: Option<String>,
    approvers: Vec<Approver>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Approver {
    name: Option<String>,
    status: Option<String>,
}
```

## 4. Write the handler

The handler is `fn(&Ctx, Args) -> anyhow::Result<Return>`. Every HTTP request,
child process and SQL batch goes through `ctx.read` or `ctx.write`. A domain
wraps that in its client (`crates/<domain>/src/client.rs`, or `lib.rs` in
k8s and sql), which also attaches its credential only to its own hosts, so
call the client's methods (`ado.get`, `ado.query`, `ado.change`) rather than
building a `Request` yourself. Here `Ado`, `list`, `text`, `stamp` and
`PREVIEW_API` come from ado's client; `Ctx`, `Effect`, `Method` and `command!`
from `agent_cli_core`:

```rust
fn approval_list(ctx: &Ctx, args: ApprovalListArgs) -> Result<Vec<ApprovalRow>> {
    let ado = Ado::load(ctx)?;
    let url = ado.api(
        Some(&ado.code_project),
        "pipelines/approvals",
        &format!("state=pending&$expand=steps&top={}", args.limit + 1),
        PREVIEW_API,
    );
    let answer = ado.get(ctx, &url)?;
    let mut rows: Vec<ApprovalRow> = list(&answer["value"])
        .iter()
        .filter_map(|approval| {
            let owner = &approval["pipeline"]["owner"];
            Some(ApprovalRow {
                id: text(&approval["id"])?,
                pipeline: text(&approval["pipeline"]["name"]),
                run_id: owner["id"].as_i64(),
                run: text(&owner["name"]),
                instructions: text(&approval["instructions"]),
                created: stamp(&approval["createdOn"]),
                approvers: list(&approval["steps"])
                    .iter()
                    .map(|step| Approver {
                        name: text(&step["assignedApprover"]["displayName"]),
                        status: text(&step["status"]),
                    })
                    .collect(),
            })
        })
        .collect();
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!("[first {}; --limit N for more]", args.limit));
    }
    Ok(rows)
}
```

Notice three conventions at work. It asks for one row more than `--limit` so
it can tell there are more, and says so on stderr with `ctx.note`, because
stdout carries data only. `stamp` passes the service's timestamp through
`agent_cli_core::utc`, so it prints as RFC 3339 UTC. Errors need no code here:
the client turns a refusal into a `Failure` with the right exit code and the
service's own words.

## 5. Register it

The `command!` macro reads the args and return types off the handler's
signature, so parsing, help and `Returns:` cannot drift from the code:

```rust
command! {
    pub APPROVAL_LIST = ["ado", "approval", "list"], Read,
    "List pending pipeline approvals (deployment gates)",
    keywords: ["pending", "waiting", "deploy", "release", "checks", "stage", "blocked"],
    example: "ado approval list --fields id,pipeline,run,approvers",
    run: approval_list,
}
```

- **Summary:** imperative, at most 80 characters, no trailing period.
- **Keywords:** words an agent would search with that are not already in the
  path, the summary or the field names.
- **Example:** starts with the path and parses with the command's own args.
  It uses `--fields` when the command returns a list of objects.
- **Timeout:** only a command that waits declares one, as `timeout: 100,`
  before `run:` (see `ado run wait`). Everything else keeps core's 60 seconds.

Then add the constant to the domain's `commands` slice in
`crates/ado/src/lib.rs`. Its position is its place in `agent-cli ado`
listings:

```rust
        pipeline::APPROVAL_LIST,
        pipeline::APPROVAL_APPROVE,
        pipeline::APPROVAL_REJECT,
```

In other crates the slice is in the `Domain` constant in `lib.rs` too. The
domain's own test counts its commands (`assert_eq!(DOMAIN.commands.len(),
29)` in ado); raise it for each command you add.

## 6. Add labeled search queries

Add at least two queries per command to `crates/cli/tests/search.toml`, in
the domain's section, written as the task an agent would be given:

```toml
[[query]]
text = "which deployments are waiting for approval"
expect = "ado approval list"

[[query]]
text = "list pending gates"
expect = "ado approval list"

[[query]]
text = "approve the production deployment"
expect = "ado approval approve"

[[query]]
text = "sign off the release gate"
expect = "ado approval approve"
```

Run the gate; for each query whose command was not first it prints `want`
and the top three it `got`:

```sh
cargo test -p agent-cli --bin agent-cli search_finds -- --nocapture
```

A new command can take another command's queries. When one does, fix the
words (summary, keywords, field names, the domain's `synonyms`) before
touching the ranker, and never delete a query to pass. For example, k8s
mapped "crash looping" to the field `restarts`, whose stem is the verb
`restart`, so `k8s deployment restart` ranked first for "worker pod crash
looping"; the synonym now points at `status` and `ready`.

## 7. Write a fixture test

Tests sit beside the code and are named as sentences. Each domain has a
`testkit` that runs the real dispatcher in process over recorded answers; the
fake transport keeps what was sent:

```rust
#[test]
fn approvals_list_the_pending_gates_and_answering_one_is_destructive() {
    let (outcome, transport) = ado(
        &["ado", "approval", "list"],
        vec![page(vec![
            json!({"id": "a-1", "status": "pending", "instructions": "Check staging",
            "createdOn": "2026-09-29T10:10:00Z",
            "pipeline": {"id": 12, "name": "web-deploy", "owner": {"id": 991, "name": "20260929.3"}},
            "steps": [{"assignedApprover": {"displayName": "Release Managers"}, "status": "pending"}]}),
        ])],
    );
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(
        outcome.json(),
        json!([{"id": "a-1", "pipeline": "web-deploy", "run_id": 991, "run": "20260929.3",
            "instructions": "Check staging", "created": "2026-09-29T10:10:00Z",
            "approvers": [{"name": "Release Managers", "status": "pending"}]}])
    );
    assert_eq!(
        urls(&transport),
        [format!(
            "{CODE}/pipelines/approvals?state=pending&$expand=steps&top=51&api-version=7.1-preview.1"
        )]
    );
    // … the dry-run half is in step 8.
}
```

Answers are synthetic or scrubbed: the repository is public, so every name is
a `contoso`-style placeholder. `testing::run`, which every `testkit` calls,
also fails the test when a printed `agent-cli …` line does not parse or a
printed time is not UTC. In a crate without a `testkit`, call it directly:
`testing::run(DOMAINS, argv, Setup::fake(FakeTransport::answering([...])))`.
`Setup::fake` sees no environment (`with_env` adds a variable) and nothing on
stdin (`with_stdin` pipes text in), and signs every `az` token as
`token@<resource>`. The k8s and azure crates run `kubectl`, `az`
and `kubelogin` from `scripts/fake`.

## 8. If the command changes something

A change goes through `ctx.write(effect, op)`, which is where `--dry-run`,
`AGENT_CLI_READ_ONLY` and `--yes` are enforced; the handler never checks those
flags itself. With the ado client that is `ado.change`:

```rust
fn answer(ctx: &Ctx, args: AnswerArgs, status: &str) -> Result<Answered> {
    let ado = Ado::load(ctx)?;
    let url = ado.api(
        Some(&ado.code_project),
        "pipelines/approvals",
        "",
        PREVIEW_API,
    );
    let body = json!([{
        "approvalId": args.id,
        "status": status,
        "comment": args.comment.unwrap_or_default(),
    }]);
    let answered = ado.change(ctx, Effect::Destructive, Method::Patch, &url, body)?;
    Ok(Answered {
        id: args.id,
        status: text(&answered["value"][0]["status"]),
    })
}

fn approval_approve(ctx: &Ctx, args: AnswerArgs) -> Result<Answered> {
    answer(ctx, args, "approved")
}
```

Its registration declares the effect, and its example includes `--yes`
because a destructive command needs it:

```rust
command! {
    pub APPROVAL_APPROVE = ["ado", "approval", "approve"], Destructive,
    "Approve a pending pipeline approval, letting the stage (a deploy) run",
    keywords: ["gate", "deploy", "release", "allow", "production", "sign", "off"],
    example: "ado approval approve 1f2e3d4c-0000-4000-8000-000000000001 --comment 'Checked staging' --yes",
    run: approval_approve,
}
```

Every `Write`, `Destructive` or `Varies` command gets a dry-run test: under
`--dry-run` the reads run, the first change is planned, and nothing that
writes reaches the transport. ado's `testkit::dry_run` asserts that and
returns the plans; in another crate use `testing::assert_dry_run(DOMAINS,
argv, answers_for_the_reads)`. The rest of the test from step 7:

```rust
    let plans = dry_run(
        &["ado", "approval", "approve", "a-1", "--comment", "Checked"],
        vec![],
    );
    assert_eq!(plans[0]["method"], "PATCH");
    assert_eq!(
        plans[0]["url"],
        format!("{CODE}/pipelines/approvals?api-version=7.1-preview.1")
    );
    assert_eq!(
        plans[0]["body"],
        json!([{"approvalId": "a-1", "status": "approved", "comment": "Checked"}])
    );

    let (outcome, _) = ado(
        &["ado", "approval", "reject", "a-1", "--yes"],
        vec![page(vec![json!({"id": "a-1", "status": "rejected"})])],
    );
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(outcome.json(), json!({"id": "a-1", "status": "rejected"}));
    let (outcome, _) = ado(&["ado", "approval", "approve", "a-1"], vec![]);
    assert_eq!(outcome.code, 2, "a gate needs --yes: {outcome:?}");
```

The registry test in `crates/cli` also runs every non-read command's example
under `AGENT_CLI_READ_ONLY` and fails if one is not refused before anything is
sent.

## 9. Record it in the contoso world

Agent trials run against `fixtures/world`. When a trial could reach the
command, and always for a command in a cross-domain trace, record the answers
it needs. Run it against the world with strict matching to see what is
missing:

```sh
env -u AGENT_CLI_FIXTURES_MATCH agent-cli ado approval list --fields id,pipeline,run
```

A miss exits 1 naming the request and the closest recording. Add the exchange
to `fixtures/world/http/ado.json`, consistent with the facts in
`fixtures/world/README.md`, and add a row there when you add a fact:

```json
{
 "request": "GET https://dev.azure.com/contoso/Fabrikam/_apis/pipelines/approvals?state=pending&$expand=steps&top=51&api-version=7.1-preview.1",
 "answer": {"count": 1, "value": [{
   "id": "00000000-0000-4000-8000-0000000000a1", "status": "pending",
   "instructions": "Apply migration 0042 to prod (adds orders.retry_count)",
   "createdOn": "2026-09-28T22:10:07.5500000Z",
   "pipeline": {"id": 15, "name": "db-migrations", "owner": {"id": 8811, "name": "20260928.1"}},
   "steps": [{"assignedApprover": {"displayName": "Release Managers"}, "status": "pending"}]}]}
}
```

Then add the call to `the_rest_of_the_world_answers_what_a_trial_is_likely_to_ask`
in `crates/cli/tests/world.rs`:

```rust
        &["ado", "approval", "list"],
```

The world's README says how exchanges match (query parameters in any order,
bodies for `POST`s).

## 10. Regenerate the reference and run the checks

`docs/reference/commands.md` is rendered from the registry, and a test fails
while it is stale, so regenerate it first (and again after changing a
summary, an argument or an example), then run the checks:

```sh
UPDATE_DOCS=1 cargo test -p agent-cli reference
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features agent-cli/fixtures -- -D warnings
cargo test --workspace
cargo test --workspace --features agent-cli/fixtures
```

Among them, `check_registry` holds the path, summary, example, flag and help
rules; the search gate needs top-1 at least 80% and top-5 at least 95%; the
overview must stay under 1 KB with every domain configured; and every command
line in the docs must parse. A `cargo test` rebuilds `target/debug/agent-cli`
without the fixtures feature, so rebuild it before trying the world again.

To confirm the command works end to end, run it against the world:

```sh
agent-cli ado approval list --fields id,pipeline,run
```

```text
[{"id":"00000000-0000-4000-8000-0000000000a1","pipeline":"db-migrations","run":"20260928.1"}]
```

## Conventions

These make one domain's output the next domain's input. The ones marked R are
checked by `check_registry` on every test run, T by `testing::run` on every
fixture test; the rest are for review. [AGENTS.md](../../AGENTS.md) states the
same rules as the contract.

| Topic | In code | Check |
|---|---|---|
| Ids | Every row whose resource has a `get` (or another one-object verb) carries `id`, exactly what that verb takes with no other flag: ado `8812`, k8s `cluster/namespace/name`, kv `vault/name`, acr `loginserver/repo:tag`, sql `schema.name`, airflow `dag/run/task/try`. The receiving verb also takes the pieces as flags and the service's web URL (parsed, never fetched); a ref and a flag that disagree is exit 2. A field naming another domain's thing holds that thing's id | review |
| Time windows | A point in time is `--since` or `--until` of type `Option<agent_cli_core::When>`; send `when.utc()` or `when.unix()`, filter on `when.0`. A future window or a length is `agent_cli_core::Span`, never named since or until. Read the clock with `agent_cli_core::now()` | R |
| Printed times | RFC 3339 UTC ending in `Z`: pass a service's stamp through `agent_cli_core::utc`, an `OffsetDateTime` through `utc_time` | T |
| Bounds | A `list` takes `--limit` (int, default 50) and says when it cut with `ctx.note`. `logs` takes `--tail`, never `--follow` | R |
| Flag names | `--since --until --limit --tail --cluster --namespace --conn`; `SYNONYM_FLAGS` refuses `--from`, `--count`, `--ns`, `--context` and the like. Never declare a global (`--fields --raw --dry-run --yes --reveal --timeout --output --no-cache`) or `-h` | R |
| Long text | A description, comment, SQL or JSON argument reads stdin when it is `-` and has a `--NAME-file` flag of type `PathBuf`; read both with `ctx.long_text(name, value, file, limit)`, one `-` per command. A cap (comments: 64 KiB) goes in the help | R |
| Scopes | A scope flag defaults to the only configured one, else exit 2 naming them: `agent_cli_core::pick` | review |
| People | A flag naming a person (`--assignee`, `--author` …) says in its help that `@me` works | R |
| Effects | Only through `ctx.read` / `ctx.write`; a `POST` that only reads is `Request::query` | read-only and dry-run tests |
| Environment | `ctx.env(name)`, never `std::env`, so tests set it with `Setup::with_env` | review |
| Secrets | Values live in `Secret`; no list or row type has a field that could hold one; only `Reveal` commands return one | review |
| Credentials | Resolved by the domain client from `KEY_env` or `KEY_cmd`, sent only to hosts that pass `host_under` | review |
| Errors | Return `Failure::usage` (exit 2), `setup` (3), `not_found` (4), `conflict` (5) or `timed_out` (124); anything else is exit 1. Add a `.hint(…)` that names the next command to run | review |
| An answer with a failing exit | `Failure::…(…).with_data(value)`: core prints `value` like a success, then the error, and exits with the failure's code (`ado run wait` on a failed run) | review |
| Hints and notes | Every `agent-cli …` they print must parse. Write placeholders in capitals (`ID`, `NAME`) and end the command where prose resumes (`, then`, ` (`, two spaces, a backtick) | T |
| Deadlines | Nothing waits past `ctx.deadline()`; a blocking call outside HTTP and child processes takes `ctx.remaining()` | review |

For every command and its arguments, see the
[command reference](../reference/commands.md). For why these rules exist, see
[The design of agent-cli](../explanation/design.md).
