# agent-cli plan

One Rust binary, `agent-cli`, that gives coding agents Azure DevOps, Azure (Key
Vault, Container Registry, AKS), Kubernetes and SQL Server/Oracle. It is not a
TUI and not for people. It starts with the CLI surface of ticket-tui, az-tui and
sql-bench (57 commands) and is built from day one to hold 1,000+.

The interface rules come from `skillbook/skills/api-cli/references/design.md`,
which measured them with no-context Sonnet and Haiku agents. This plan ports
that runtime (`assets/cli.py.tmpl`) to Rust and applies it to hand-written
commands.

## Decisions to confirm

| # | Decision | Recommendation | Why |
|---|---|---|---|
| 1 | Where the domain code lives | **Port it into agent-cli crates.** The TUIs keep their copies for now; later they depend on agent-cli's crates (phase 5) | Extracting shared crates first would block this project on refactoring three TUIs. Their core code also leaks ratatui (`config.rs` `Color`, `model` → `columns`), and that has to be cut during the port anyway |
| 2 | ADO reads: live or a local SQLite cache like ticket-tui | **Live** (WIQL and REST), with a small JSON cache for IDs only | With a cache, every read depends on a prior `sync`, the answers can be stale, and ticket-tui's schema-v19 DB gets wiped when versions differ. An agent wants correct answers more than 300 ms back |
| 3 | Secret values (`kv secret get`, `k8s secret get`) | **Need `--reveal`.** `--output FILE` writes the value to a 0600 file and prints only the path | Anything a command prints lands in the agent's transcript |
| 4 | The TUIs' own CLIs after parity | **Remove them in phase 5** | One agent surface, one place to fix bugs |
| 5 | First domain to port | **sql.** It's the smallest (~4.5k lines) and can be tested locally with sql-bench's compose containers | Shakes out the runtime before the 15k-line ADO port |
| 6 | Crate name | Keep `agent-cli` as the repo and binary name. **Never publish to crates.io**: the name is taken there (0.1.1) | Install with `cargo install --git` |

## Command map

Every command is `agent-cli <domain> <resource> <verb>`: always three words.
Domains are services. Verbs come from a closed list that a test enforces.

### ado (from ticket-tui)

| New | From | Effect |
|---|---|---|
| `ado workitem list` | `list --query` | read (WIQL; typed filters plus `--wiql`) |
| `ado workitem get <id>` | `show` | read (description and acceptance criteria as Markdown, plus `rev`) |
| `ado workitem create` | `create` | write |
| `ado workitem update <id>` | `edit` | write (`--if-rev N` for optimistic concurrency) |
| `ado workitem comment <id>` | `comment` | write (`-` reads stdin) |
| `ado workitem link <id> --repo R [--branch B]` | `link` | write (creates the branch if missing) |
| `ado team list` | `teams` | read |
| `ado repo list` / `get` | `repos list/show` | read (live; no local-clone scan) |
| `ado pr list` / `get` | `prs list/show` | read (**all PRs**, not only repos with a local clone) |
| `ado pr create` | `prs create` | write (idempotent: reuses the open PR and repairs its links) |
| `ado pr vote <id> <vote>` | `prs vote` | write |
| `ado pr update <id>` | `prs autocomplete` | write (`--autocomplete on\|off`, `--draft`, `--title`) |
| `ado pr link <id> --workitem N` | `prs link` | write |
| `ado pr comment <id>` | `prs comment` | write |
| `ado pr complete <id>` | `prs complete` | destructive |
| `ado pr abandon <id>` | `prs abandon` | destructive |
| `ado pipeline list` | `pipelines` | read (live; no clone filter) |
| `ado run list` / `get` | `runs list/show` | read |
| `ado run logs <id>` | `runs logs` | read (bounded `--tail`; no `--follow`) |
| `ado run create --pipeline P --branch B` | `runs trigger` | write |
| `ado run wait <id>` | `runs wait` | read (bounded by `--timeout`, see time budgets) |
| `ado run cancel` / `retry <id>` | `runs cancel/retry` | destructive / write |
| `ado approval list` | `approvals list` | read |
| `ado approval approve` / `reject <id>` | `approvals approve/reject` | destructive (a gate can deploy to production) |

### kv, acr, aks, k8s (from az-tui, TUI-only features included)

| New | From | Effect |
|---|---|---|
| `kv vault list` | inventory inside doctor | read (Resource Graph) |
| `kv secret list [QUERY]` | `secrets` | read (metadata only; a test proves no value can appear) |
| `kv secret get <name> --reveal` | `secret get` | reveal |
| `kv version list <secret>` | TUI `vault::versions` | read |
| `acr registry list` | inventory | read |
| `acr repo list [QUERY]` | `repos` | read (**tag counts and dates filled** via `acr::attributes`; az-tui's CLI returned nulls) |
| `acr tag list <repo>` | `tags` | read |
| `acr manifest get <repo> <tag\|digest>` | TUI `acr::manifest` | read |
| `aks cluster list` | `setup` discovery | read |
| `aks cluster connect <name>` | `setup` | write (`az aks get-credentials` plus `kubelogin convert`; changes kubeconfig) |
| `k8s context list` | config scopes | read |
| `k8s pod list` / `get <pod>` | TUI `pods` / `describe` / `yaml` | read (`--yaml` for the manifest) |
| `k8s pod logs <pod>` | TUI `logs` | read (`--tail 200`, `--previous`, `--container`; no follow) |
| `k8s pod delete <pod>` | TUI `delete_pod` | destructive |
| `k8s event list [--pod P]` | TUI `events` | read |
| `k8s configmap list` / `get` | TUI `configmaps` | read |
| `k8s secret list` | TUI `secrets` | read (key names and sizes only) |
| `k8s secret get <name> <key> --reveal` | TUI `secret_value` | reveal |
| `k8s deployment restart` / `scale` | TUI `rollout_restart` / `scale` | destructive |

Every k8s command takes `--cluster` and `--namespace`. They default to the only
configured scope when there is exactly one; otherwise, leaving them out is a
usage error that lists the choices. This is a thin, bounded, JSON-emitting
kubectl wrapper, not a kubectl replacement.

### sql (from sql-bench)

| New | From | Effect |
|---|---|---|
| `sql connection list` | config | read (names, kind, host, `read_only`; never credentials) |
| `sql query run --conn C <SQL\|->` | `query` | read or write, classified per statement |
| `sql query bench --conn C <SQL\|->` | `bench` | same classification |
| `sql object list --conn C [PATTERN]` | `objects` | read (now with a timeout and a row cap) |
| `sql object get --conn C <schema.name>` | `source` | read (source text, or columns plus a DDL sketch for a table) |
| `sql schema list --conn C` | TUI `catalog::list_schemas` | read |

### Built-ins (reserved words; a test checks that no domain uses them)

`agent-cli` (overview), `agent-cli search <words>`, `agent-cli doctor [domain]`,
and `--help` on any path.

### Not ported

| Command | Reason |
|---|---|
| ticket-tui `sync` | No local DB (decision 2) |
| ticket-tui `status` | A status-bar badge line built from TUI session files |
| ticket-tui `agent launch/prompt/list` | Herdr orchestration. `ado workitem get` already gives an agent the content |
| az-tui `setup --write` | Writing config.toml. Agents get `aks cluster list` and the README shows the TOML |
| sql-bench TUI replay flags, `--format table/csv` | Human and test-harness features. Output is JSON only |

## Design for 1,000+ commands

### Discovery

- **The overview stays under 1 KB at any size.** It lists domains, not
  groups, and does no network or process calls:

  ```
  agent-cli: Azure DevOps, Key Vault, ACR, AKS, Kubernetes, SQL for agents — 57 commands. Output: JSON.
  Start here:  agent-cli search <what you want to do>    e.g. agent-cli search "my active work items"
  Browse:      agent-cli <domain> [<resource>]    Details: agent-cli <domain> <resource> <verb> --help
  Flags:       --fields a,b.c  --raw  --dry-run  --yes  --timeout S  --output FILE
  Exit:        0 ok · 1 failed · 2 fix the call · 3 needs setup (run doctor) · 4 not found · 5 conflict · 124 timed out
  Config:      ado ✓ · kv/acr ✓ · k8s 2 scopes · sql 3 connections    Live check: agent-cli doctor
  Now:         2026-09-29T18:40Z
  Domains:     ado(29)  kv(4)  acr(4)  aks(2)  k8s(12)  sql(6)
  ```

- **Listings.** `agent-cli <domain>` lists resources with their verbs.
  `agent-cli <domain> <resource>` lists verbs with summaries. Above 40 entries a
  listing shows names only (design.md default 1).
- **Search is the front door.** It is a port of the api-cli ranker: BM25F over
  path ×3, summary ×2, arg names, return fields and keywords, times squared
  term coverage, plus a stemmer, prefix matching and synonyms. On top of that
  it adds **domain synonyms** (ticket/bug/story/task → workitem, pull request →
  pr, build/ci/pipeline → run, image → acr, container → pod,
  password/credential → secret, table/view/proc → object). It prints six hits,
  each with its required args. At 1,000 in-memory documents it needs no index.
- **Help per command** (under 2 KB, enforced): summary, args (`*` marks
  required), effect, `Returns:` field shape, and one runnable example that
  uses `--fields`:

  ```
  agent-cli ado workitem list — List work items matching filters (live WIQL)
   --assignee str      name, email or @me
   --state str[]       e.g. Active, "In Progress"
   --type str[]        Bug, "User Story", Task …
   --iteration str     path or @current
   --text str          words in title or description
   --wiql str          raw WIQL WHERE clause, ANDed with the rest
   --limit int         (default 50)
  Returns: [{id,type,title,state,assignee,iteration,area,priority,tags[],changed,rev}]
  Read. Globals: --fields --raw --timeout
  e.g. agent-cli ado workitem list --assignee @me --state Active --fields id,title,state
  ```

- **Wrong names get "did you mean"** plus the closest search hits. A missing
  arg shows the example.

### Output contract (the same for every command)

- stdout carries data only, as JSON. It is compact when piped and pretty on a
  TTY. Nulls and empties are dropped. There are no `--json` or `--format`
  flags.
- `--fields a,b.c` projects nested fields through lists. When a projection
  matches nothing, stderr lists the available paths.
- **Output guard at 12 KB.** It keeps a prefix of the largest list, prints
  valid JSON, and saves the full output to a temp file. stderr names that
  file, the item count and suggested `--fields`. `--raw` bypasses the guard.
- List commands default to `--limit 50`. When there is more, stderr says
  `[50 of 312; --limit N]`.
- Text payloads (logs, SQL source) come back as `{"text": …}` plus metadata,
  with the same guard (tail for logs, head for source).
- **The SQL result shape is fixed:**
  `{"results":[{"columns":[…],"types":[…],"rows":[[…]],"rows_affected":n,"truncated":bool}],"elapsed_ms":n}`.
  This replaces sql-bench's array-of-objects vs array-of-arrays split, carries
  column names and types even for zero rows, and doesn't repeat the keys on
  every row.
- **Values:** timestamps are RFC 3339 with `T` on both databases. Integers
  above 2^53 and decimals are strings, and decimals keep their scale.
  Binary is `0x…`.

### Errors and exit codes

- stderr gets `error: <what>` and, when there is one, `hint: <next command to
  run>`. Messages are redacted: tokens, passwords, and PAT or Bearer values
  never appear.

  | Code | Meaning | Examples |
  |---|---|---|
  | 0 | ok | |
  | 1 | failed | a remote error, or `run wait` finished and the run did not succeed |
  | 2 | usage: fix the call | unknown path or flag, bad value, an ambiguous name |
  | 3 | needs setup | not signed in, no config section, kubectl or Instant Client missing |
  | 4 | not found | |
  | 5 | conflict | revision mismatch, PR already completed |
  | 124 | timed out | |
  | 130 | interrupted | |

- This removes ticket-tui's clash of `runs wait` exit 2 (canceled) with
  clap's usage exit 2. The outcome of a wait is in the JSON.

### Safety

- **Every command declares an effect:** `read`, `write`, `destructive` or
  `reveal`. Help and search show it.
- **Writes happen in one place.** Handlers call `ctx.read(req)` or
  `ctx.write(req)`; the second covers HTTP, kubectl argv and SQL alike.
  `write` enforces:
  - `--dry-run`: prints the planned request (redacted) and stops before the
    first write. Reads before it still run.
  - `AGENT_CLI_READ_ONLY=1`: refuses every write, destructive and reveal
    command. POSTs that only read (WIQL, Resource Graph, ACR token exchange)
    go through `read`.
  - `--yes` for destructive commands. Without it the command exits 2 with the
    exact command to re-run.
- **Secret handling** (ported from az-tui):
  - Credentials live in a `Secret` newtype with no `Serialize`, and
    `Debug`/`Display` print `[redacted]`.
  - Each token goes only to its audience host (`*.vault.azure.net`,
    `*.azurecr.io`, `dev.azure.com`), checked on every URL including
    `nextLink`.
  - HTTP never follows redirects.
- **SQL writes.**
  - A conservative lexer classifies each statement. Only a leading
    `select`/`with` counts as a read; everything else is a write and needs
    `--yes`.
  - A connection with `read_only = true` refuses writes outright.
  - The README says the real guard is a read-only database login.

### Time budgets

- Claude Code's Bash tool times out at 2 minutes, so every command has a
  deadline: default 60 s, set with `--timeout`.
- Throttle waits are capped by that deadline. This fixes az-tui sleeping up
  to 3,600 s on a 429.
- `ado run wait` defaults to 100 s. On timeout it exits 124 and prints the
  run's current state, with a hint to run it again.
- Child processes (`az`, `kubectl`, `kubelogin`) use az-tui's `run_until`:
  - stdin is null
  - both pipes are drained
  - the child gets its own process group, which is killed at the cap, so a
    device-code prompt can't hang the command

### Config, auth, cache

- **Config file:** one file, `$XDG_CONFIG_HOME/agent-cli/config.toml`
  (`AGENT_CLI_CONFIG` overrides the path). Its sections:
  - `[ado]` org, project, code_project, team
  - `[azure]` subscriptions, vaults, registries, parallel
  - `[[k8s.scope]]` name, context, namespaces
  - `[sql]` oracle_client_dir, and `[[sql.connection]]` name, kind, host,
    port, database / service, user, `password` / `password_env` /
    `password_cmd`, trust_cert, encrypt, read_only (inside `[sql]`, so a
    broken Oracle setting can only break sql)
- **Config loading:**
  - Each domain parses only its own section, lazily. A bad `[sql]` section
    can't break `ado` (in ticket-tui and sql-bench, any config error kills
    every command).
  - `AGENT_CLI_<SECTION>_<KEY>` overrides any scalar key.
  - `[ado]` falls back to `az devops configure` defaults, as ticket-tui does.
- **Auth:** `AZURE_DEVOPS_EXT_PAT`, else `az account get-access-token
  --resource …` per audience, re-minted once on a 401.
  - **Measure the `az` token call in phase 1.** If it costs more than
    ~300 ms per invocation, add an on-disk token cache (0600, expiry-aware),
    because agents make many short calls.
- **Cache:** one small JSON key-value store in `$XDG_CACHE_HOME/agent-cli/`
  (0600, TTL per entry) for slow-changing lookups:
  - ADO project, repo and user IDs and identities
  - the Resource Graph inventory
  - ACR repository attributes

  `--no-cache` bypasses it. It never holds secret values.

## Architecture

```
agent-cli/
  Cargo.toml            workspace: shared deps, [workspace.lints], release profile
  crates/core/          registry, dispatch, help/overview/search, output, errors,
                        config, cache, process runner, HTTP transport, Secret
  crates/ado/           Azure DevOps
  crates/azure/         az tokens, Resource Graph, kv, acr, aks
  crates/k8s/           kubectl wrapper
  crates/sql/           tiberius + oracle; the only crate with tokio
  crates/cli/           bin `agent-cli`: main() = core::run(&[ado::COMMANDS, azure::COMMANDS, …])
```

A crate per domain keeps incremental builds fast as commands pile up. It also
stops domains from reaching into each other: tiberius/tokio and oracle's C
build stay out of everything else.

### A command is static data

```rust
pub struct Command {
    pub path: [&'static str; 3],            // domain, resource, verb
    pub summary: &'static str,              // ≤ 80 chars, imperative
    pub effect: Effect,                     // Read | Write | Destructive | Reveal
    pub keywords: &'static [&'static str],  // extra search terms
    pub example: &'static str,              // parsed by a test; must use --fields when it returns rows
    pub args: fn() -> clap::Command,        // <A as clap::Args>::augment_args
    pub returns: fn() -> schemars::Schema,  // schema_for!(R)
    pub run: fn(&Ctx, &clap::ArgMatches) -> Result<serde_json::Value>,
}

// One macro ties the typed handler to the entry, so args, help and Returns can't drift:
command! {
    WORKITEM_LIST = ["ado", "workitem", "list"], Read,
    "List work items matching filters (live WIQL)",
    keywords: ["ticket", "bug", "story", "backlog", "assigned"],
    example: "ado workitem list --assignee @me --state Active --fields id,title,state",
    fn run(ctx: &Ctx, a: ListArgs) -> Result<Vec<WorkItemRow>> { … }
}
// each domain: pub const COMMANDS: &[Command] = &[workitem::LIST, workitem::GET, …];
```

How the pieces fit:
- **clap parses only the one leaf** that is invoked. Its own help and
  subcommands are off.
- **core renders all help** from clap's `Arg` metadata and the `returns`
  schema. Parsing, help and the Returns line therefore come from the handler's
  own types, so a command can't document a field it doesn't return.
- **Positional args work as design.md intended** (`ado pr get 123`), because
  the handler's clap struct declares them.
- **Registration is explicit.** Adding a command means one `command!` block
  plus one line in its domain's `COMMANDS`. Tests catch the rest. No `linkme`
  or `inventory` magic.

### Dependencies

| Crate | Why |
|---|---|
| `clap` 4 | Typed leaf parsing. All three sources use it, so their arg structs port as they are |
| `schemars` 1 | `Returns:` lines and search over return fields. Hand-written field lists would drift across 1,000 commands |
| `serde`, `serde_json`, `toml` | |
| `anyhow` | Plus a typed `Failure { code, hint }` that core downcasts to pick the exit code |
| `ureq` 3 (rustls) | Blocking HTTP, so 90% of commands never start an async runtime. Both Azure sources already use it with redirects off and body caps |
| `time` | The ported date code already formats with it |
| `strsim` | "Did you mean" suggestions; already in the tree via clap |
| `tempfile` | The output guard's saved copies |
| sql only: `tiberius` 0.12 (tds73, rustls), `tokio` (rt, net, time), `tokio-util`, `futures-util`, `oracle` 0.6 | |

**Constraints:**
- Keep `panic = "unwind"`: tiberius panics on some types, and the worker
  catches it.
- Build dynamically against glibc (no static musl), because ODPI-C `dlopen`s
  Instant Client at run time. That's why the binary starts without it.
- The build needs a C compiler (`odpic-sys`).
- Edition 2024, rust-version 1.88, MIT, thin LTO and stripped symbols, the
  same as the sources.

## Behaviour fixes carried in the port

| Source | Problem | In agent-cli |
|---|---|---|
| ticket-tui | Reads need a prior `sync`; a DB with another schema version is dropped | Live reads, no DB |
| ticket-tui | `prs list`, `pipelines` and `status` hide anything without a local clone | No clone filter |
| ticket-tui | `list` has no limit (tens of thousands of rows) | `--limit 50`, plus the guard |
| ticket-tui | `runs show` swallows timeline errors | Errors surface |
| az-tui | CLI `repos` returns null counts and dates | Filled via `acr::attributes`, then cached |
| az-tui | `--refresh` means seconds globally but is a switch on subcommands; `--vault` means two things | No TUI flags. Every flag has one meaning, checked by a test that compares flag names and types across commands |
| az-tui | A throttle wait of up to 1 h | Capped by the command's deadline |
| sql-bench | A blank line splits statements (breaks `declare`, blank line, `select`) | Split only on `GO` (mssql), and on `;` or `/` (oracle) |
| sql-bench | `objects` and `source` have no timeout or row cap | The global deadline and `--limit` |
| sql-bench | Date separators differ (`T` vs space) | RFC 3339 on both |
| sql-bench | config.example.toml gets the Oracle env-var precedence wrong | One documented order: env, then config, then system |

## Testing

1. **Registry invariants.** This is what keeps 1,000 commands consistent. Each
   item is one test over `COMMANDS`:
   - Paths are unique and three words long; the domain is known; the verb is
     in the vocabulary; no built-in word is shadowed.
   - The summary is ≤ 80 chars.
   - **The example parses** with the command's own clap args, and uses
     `--fields` when the command returns a list.
   - Each flag name has one type and one meaning across all commands.
   - Help is ≤ 2 KB, the overview ≤ 1 KB, and listings of more than 40 show
     names only.
   - Under `--dry-run` with a recording transport, no write or destructive
     command reaches the transport.
   - With `AGENT_CLI_READ_ONLY=1`, every non-read command is refused.
2. **Search quality.** `tests/search.toml` holds labeled queries written as
   tasks, e.g. "who approved PR 42" → `ado pr get`.
   - **Every new command adds at least two queries.**
   - The CI gate is top-1 ≥ 80% and top-5 ≥ 95%. The api-cli ranker measured
     26/40 top-1 and 38/40 top-5 on GitHub before domain synonyms.
3. **Performance.** The registry is padded to 1,000 synthetic commands, and
   the test times the overview, search and help.
   - Measure first, then set the gates just above the baseline.
   - If search over live `args()` and `returns()` gets slow, precompute the
     index in `build.rs`.
4. **Domain tests.**
   - A `Transport` trait with scrubbed JSON fixtures covers ADO, Key Vault,
     ACR and Resource Graph.
   - az-tui's `scripts/fake-kubectl` covers k8s.
   - sql-bench's `compose.yaml` and seed scripts cover sql, gated by
     `AGENT_CLI_TEST_DBS`. SQL Server also runs in CI as a service container.
   - The `oracle_no_client` test is kept, since it checks the binary starts
     and fails cleanly without Instant Client.
5. **Agent trials (acceptance for each phase).** Use design.md's method:
   - fresh Sonnet 5.5 and Haiku 4.5 subagents
   - the prompt names only the tool and the task
   - a shim logs argv, exit codes and output sizes
   - record tokens, calls and correctness for each task

   Target at least 6 tasks per domain. Every miss becomes a fix, a test, or a
   search query.

CI (GitHub Actions, Linux and macOS): fmt, clippy `-D warnings`, test, and a
release build.

## Phases

| Phase | Scope | Done when | Status (2026-09-30) |
|---|---|---|---|
| 0. Repo | Workspace, core, CI, README stub, AGENTS.md with the "adding a command" checklist | CI green on an empty registry. Overview, search, help and guard all tested against synthetic commands | Done |
| 1. sql | Port about 4.5k lines: drivers, catalog, splitter (without blank-line splitting), export → the result shape above. `doctor sql` | All 6 sql commands pass against both compose DBs. Agent trial of 6 tasks. `az` token latency measured (for phase 2) | Done; the signed-in `az` latency is still to measure (TODO.md) |
| 2. ado | Port about 13–15k lines: azure client, model, markdown and html, edit, classification. WIQL-backed list. Drop sync, db, local, watch, agents and status | 29 commands. Fixture tests. Read-only trial against a personal ADO org (no employer names in the repo). Agent trial of 8 tasks | Done against fixtures; the live read-only run is pending |
| 3. kv, acr, aks, k8s | Port about 5.5k lines: auth, transport, graph, vault, acr, kube (no watcher). Plus the TUI-only features in the map | 22 commands. fake-kubectl and fixture tests. **First run against real Azure and a real cluster**, using az-tui's `plan/CHECKLIST.md`, since az-tui has never had one | Done against fixtures (23 commands with `k8s deployment list`); the live run is pending: `docs/first-live-run.md` |
| 4. Harden | Search tuning from the trial transcripts, perf gates, README (Diátaxis: tutorial, how-to per domain, reference generated from the registry) | Search gates met. A one-line global CLAUDE.md note tells agents that agent-cli exists | Done, except the global CLAUDE.md note (the README gives the line). Added along the way: the cross-domain conventions (`docs/plans/cross-domain.md`), the airflow (15 commands) and dd (20 commands) domains, and the contoso world. Trial of 12 tasks on both models, 24 of 24 correct (`docs/trials/2026-09-30.md`); docs in `docs/` |
| 5. Consolidate (optional) | The TUIs depend on the agent-cli domain crates via git and delete their copies and their CLIs | Each TUI's tests pass on the shared crates | Not started; waits for the live runs of phases 2 and 3 |

## Growth path to 1,000+

Hand-written commands stop somewhere in the low hundreds. The bulk arrives
from API specs:

- api-cli's `build.py` already turns OpenAPI and AsyncAPI into a catalogue
  (`commands.json`) with params, returns and summaries. Azure DevOps and ARM
  both publish OpenAPI specs.
- When the first spec domain lands, add one variant to `Command.run`: a
  generic HTTP runner that reads an embedded catalogue entry. It gets search,
  help, `--fields`, the guard, dry-run and read-only for free, because those
  live in core.
- Until then, don't build it. Nothing in the registry has to change for it
  to fit.
- Compile time stays bounded: generated commands are data, not a clap derive
  each.

## Public-repo rules

- **No org, project, subscription, vault, registry, cluster or server names**
  anywhere: code, fixtures, docs, commit messages or trial logs. Fixtures are
  synthetic or scrubbed.
- `config.toml` is never committed. `config.example.toml` uses `contoso`-style
  placeholders.
- The sql compose passwords are throwaway and localhost-only, and are
  labelled that way, as in sql-bench.
- The GitHub repo is created and pushed only when you say so.

## Risks

- **az-tui's Azure half has never run against real Azure.** Phase 3 is its
  first live run, so budget for fixes there.
- **ADO fixtures could leak employer data.** Scrub them, and capture from a
  personal org where possible.
- **Oracle on macOS** needs an arm64 Instant Client and a C compiler. The
  Oracle tests run where the client exists and are skipped elsewhere.
- **The code drifts** between the TUIs and agent-cli until phase 5.
- **`az` start-up cost** (a Python process per token) may dominate short
  calls. It gets measured in phase 1, with the token cache as the fix.
