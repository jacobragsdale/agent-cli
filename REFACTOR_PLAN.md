> Done on 2026-09-30; results in docs/trials/dev-after-2026-09-30.md. Two criteria were not met: D2 still edits scripts/fake/kubectl, and tokens fell 18% on two runs per task (29% on the one run the plan specifies).

# Refactor plan: small units of work for agents

This plan restructures agent-cli so an agent (or a person) adding or changing a
command reads and touches a small, predictable set of files, however large the
registry grows. It is written for an agent starting fresh. Work through it top to
bottom, tick the boxes as you go, and commit after each numbered step.

**No command's behaviour changes anywhere in this plan.** Every phase except 0
and 4 is a pure restructure. `agent-cli`'s overview, every `--help`, the search
results and the generated reference must be byte-identical before and after,
except where a step says otherwise.

## 0. Read this first

- **Read:** `AGENTS.md`, which `CLAUDE.md` imports and which loads
  automatically. Then `docs/how-to/add-a-command.md` and
  `docs/how-to/add-a-domain.md`. Skim `crates/core/src/lib.rs` (the core
  facade), and one domain crate, e.g. `crates/sql`, end to end.
- **Don't read:**
  - `PLAN.md` and `docs/plans/*`: the original design and research, now
    history.
  - `docs/reference/commands.md`: generated, 73 KB. Use
    `agent-cli <domain> <resource> <verb> --help` instead.
  - `fixtures/world/http/*.json` whole: grep them by URL.
  - `Cargo.lock`.
- **Learn existing commands from the binary**, the same way agents do:
  - `cargo build -p agent-cli && target/debug/agent-cli` for the overview.
  - `agent-cli search "<words>"`, `agent-cli <domain>`, and
    `agent-cli <domain> <resource> <verb> --help`.
- **Rules that still apply:**
  - The repo is **public**: contoso placeholders only.
  - Commit subjects are descriptive sentences (see `git log`).
  - Never name a handler `run`, `args` or `returns`, because the `command!`
    macro's inner functions shadow them.
  - Push only when the user asks.
- **This machine:** about 3 GB of RAM is free. Run at most two cargo builds at
  once. `CARGO_BUILD_JOBS=4` helps when agents work in parallel.
- **Leave alone:** the `~/dev/*-handoff` worktrees belong to another task.

### The checks (CI's steps; run before every commit)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features agent-cli/fixtures -- -D warnings
cargo test --workspace
cargo test --workspace --features agent-cli/fixtures
```

- **Baseline on 2026-09-30 (commit `88156b8`):**
  - 368 tests pass with fixtures.
  - The search gate finds the right command first for 253 of 253 queries, and
    in the top five for 253 of 253.
- **Tests that must never regress:** the gate tests in
  `crates/cli/src/main.rs`:
  - `the_registry_keeps_every_rule`
  - `search_finds_the_labeled_command`
  - `read_only_mode_refuses_every_change`
  - `the_overview_fits_with_every_domain_configured`
  - `the_command_reference_matches_the_registry`
  - `every_command_line_in_the_docs_parses`
- **SQL database tests** need `AGENT_CLI_TEST_DBS=1` and the compose
  containers (`scripts/db-up.sh`). Run them in Phase 3 for the sql crate; they
  skip otherwise.

## Why: what an agent pays today (measured 2026-09-30)

| Cost | Now |
|---|---|
| Largest source files | `ado/workitem.rs` 1,977 lines (7 commands); `ado/pr.rs` 1,782 (11); `ado/pipeline.rs` 1,628 (11); `azure/acr.rs` 1,342; `azure/kv.rs` 1,288; 10 more over 800 |
| Domain `lib.rs` | 330–777 lines: registration mixed with status, doctor and shared code |
| Always-loaded context | `AGENTS.md` 10 KB. It points at `PLAN.md` (27 KB), which agents then read |
| Shared files touched by one change | The parity-filters commit touched 8 files outside its domain: `search.toml`, `world.rs`, the world README, `ado.json`, `registry.rs`, `search.rs`, the reference and `TODO.md`. Adding the dd domain touched 7. The two parallel domain builds conflicted in 10 files |
| Generated reference | One 73 KB file at 93 commands, about 800 KB at 1,000 |

**The target:** a routine new command in an existing domain touches only:
- `crates/<crate>/`
- `fixtures/world/http/<domain>.json`, if it needs a recording
- its domain's generated reference page

Agents learn the surroundings from a short domain card, the core card, and the
binary's own help.

## Phase 0: measure the baseline (before changing anything)

The same trial method as `docs/how-to/run-agent-trials.md`, applied to
*development* tasks instead of CLI use.

### 0.1 A stats script

- [x] Add `scripts/context-stats.sh`. It prints:
  - the 15 largest `.rs` files under `crates/*/src` (lines)
  - lines per crate
  - for the last N commits (default 20), how many files each touched outside
    the domain crate its subject names

  Plain shell; `git` and `wc` only.
- [x] Run it and paste the output into `docs/trials/dev-baseline-2026-09-30.md`.

### 0.2 Three development tasks, run by fresh agents

Each task runs in its own **throwaway** worktree (`git worktree add
../agent-cli-trial-D1 -b trial-D1 main`). **Never merge trial branches.** They
exist only to be measured, and are deleted afterwards. Run every task with
Sonnet 5.5; add Haiku 4.5 if time allows.

The prompt is the same for every run, with the worktree path and the task
filled in:

> You are working in the git worktree at `<path>`, a checkout of agent-cli.
> Task: `<task>`. Implement it, make every repo check pass, and commit. Don't
> push.

| Id | Task (as given to the agent) | What a correct result has |
|---|---|---|
| D1 | Add `ado pipeline get <id>`: one pipeline definition by id or name, with its id, name, folder, repository, default branch, YAML path, queue status and web URL | A Read command, args, return struct, handler, fixture test, 2+ search queries, a world recording, and the reference regenerated |
| D2 | Add a `--label key=value` filter (repeatable) to `k8s deployment list`, passed to kubectl as a label selector | A flag on an existing command, a fixture test proving the selector reaches kubectl, a search query, the reference regenerated |
| D3 | Add `airflow run cancel <id>`: mark a queued or running DAG run failed (Airflow 3: `PATCH /api/v2/dags/{dag_id}/dagRuns/{dag_run_id}` with state `failed`). It is destructive | A Destructive command, a dry-run test, read-only refusal, a fixture test, search queries, the reference regenerated |

For each run, record:
- tokens, tool calls and wall time (from the subagent's usage report)
- whether the five checks pass on the committed tree (run them yourself)
- `git diff --name-only main...trial-Dn`: which files changed, and how many
  are outside the task's crate
- `git diff --shortstat`

- [x] Run D1–D3 and record every row in
  `docs/trials/dev-baseline-2026-09-30.md`: the table plus a short note on
  where each agent spent its reading, taken from its report.
- [x] Delete the trial worktrees and branches.
- [x] Commit the stats script and the baseline doc.

## Phase 1: each domain owns its files

Goal: nothing a routine command change needs lives in a file shared by all
domains. Do the steps in order and run the checks after each.

### 1.1 Workspace members by glob

- [x] `Cargo.toml`: `members = ["crates/*"]`.
- [x] **Verify:** `cargo metadata --no-deps --format-version 1 | grep -o
  '"name":"agent-cli[^"]*"' | sort -u` lists all 8 crates.

### 1.2 Search queries per crate

- [x] **Split the file:** move `crates/cli/tests/search.toml` into
  `crates/<crate>/search.toml`, one per crate. The azure crate's file holds
  kv, acr and aks. A query that tests a cross-domain reading (see
  `SHARED_WORDS` in `crates/core/src/registry.rs`) goes in the crate of the
  command it expects.
- [x] **Read them at runtime:** `search_finds_the_labeled_command` in
  `crates/cli/src/main.rs` walks `crates/*/search.toml` with `std::fs`
  (sorted, via the repo root it already uses as `REPO`) and concatenates
  them. A new crate's file is then picked up with no edit here.
- [x] **Verify:**
  - The gate reports the same counts (253/253 top-1 and top-5), and the total
    query count is unchanged.
  - `crates/cli/tests/search.toml` is gone.
- [x] Update the "Adding a command" step in `AGENTS.md` and
  `docs/how-to/add-a-command.md` to name `crates/<crate>/search.toml`.

### 1.3 The generated reference per domain

- [x] **Change the generator.** It lives in `crates/cli/src/main.rs`
  (`reference()`, `the_command_reference_matches_the_registry`). It now writes
  `docs/reference/<domain>.md` for each domain, plus `docs/reference/README.md`:
  an index with the one-paragraph header, the global flags and exit codes, and
  one row per domain (name, summary, command count, link).
- [x] **Keep the regeneration command:** `UPDATE_DOCS=1 cargo test -p
  agent-cli reference` still regenerates everything. The staleness test covers
  every file, and fails on a stray `docs/reference/*.md` that no domain
  produces.
- [x] **Delete** `docs/reference/commands.md` and fix every link to it
  (`grep -rn "reference/commands.md" docs README.md AGENTS.md`).
- [x] **Verify:**
  - Every `## <domain>` section of the old file equals the body of its new
    file (spot-check two domains with `diff`).
  - `every_command_line_in_the_docs_parses` passes.

### 1.4 World tests per domain

- [x] **Split the file:** `crates/cli/tests/world.rs` (543 lines, all domains)
  becomes:
  - `crates/cli/tests/common/world.rs`: the `agent_cli`, `ok` and env helpers.
  - one `crates/cli/tests/world_<domain>.rs` per domain, holding that
    domain's checks from `the_rest_of_the_world_answers_what_a_trial_is_likely_to_ask`
    and `the_world_answers_the_filters_the_old_clis_had`.
  - `crates/cli/tests/world_cross.rs`: the traces that span domains
    (`the_deploy_trace_runs_from_the_cluster_to_the_work_items`,
    `last_nights_failed_dag_run_leads_to_its_exception_and_its_pod`,
    `datadog_shows_the_alert_at_the_deploy_the_errors_and_the_pod_to_hop_to`),
    and the miss-message test.
- [x] **Verify:** `cargo test -p agent-cli --features fixtures` runs the same
  number of world tests, and each command line the old file ran is still run
  exactly once. Compare the lists: `grep -c 'ok(&\[' ` before and after,
  summed.

### 1.5 Stand-in credentials live in the world, not the script

- [x] **Move the credentials.** `scripts/trial-env.sh` exports
  `AIRFLOW_PROD_PASSWORD` and `DD_ACCESS_TOKEN`, and `world.rs` sets them.
  Move them into `fixtures/world/config.toml` as `password_cmd = "echo
  stand-in"` (airflow) and `token_cmd = "echo fixture-dd-token"` (dd).
  Credentials already accept `*_cmd`. Remove the exports and the `.env(...)`
  lines.
- [x] **Verify:** a new domain now needs no edit to `scripts/trial-env.sh`.
  Run `eval "$(scripts/trial-env.sh)"`, then `target/debug/agent-cli airflow
  dag list` and `agent-cli dd monitor list` (built with fixtures). Both
  answer.

### 1.6 World facts per domain

- [x] **Split the table:** the "What is true here" table in
  `fixtures/world/README.md` moves to `fixtures/world/facts/<domain>.md`, one
  per domain. The README keeps how the world works, how to extend it, and a
  list of links to the facts files.
- [x] Update `docs/how-to/add-a-domain.md` and the world README's "Extend it"
  steps.

### 1.7 Check Phase 1

- [x] All five checks pass.
- [x] **Overview and help unchanged:** `target/debug/agent-cli` is
  byte-identical to before Phase 1. So is `agent-cli ado pr vote --help`
  (keep a copy from before the phase and `diff` them).
- [x] **Walk it through:** confirm on paper that adding a command to `dd`
  needs only `crates/dd/**`, `fixtures/world/http/dd.json`,
  `fixtures/world/facts/dd.md`, `crates/cli/tests/world_dd.rs` (only when it
  adds a world check) and `docs/reference/dd.md`. Write that list into
  `docs/how-to/add-a-command.md`.
- [x] Commit each step separately; the subjects say what moved.

**What stays shared, on purpose:**
- `crates/cli/src/main.rs` `DOMAINS`: one line per domain.
- `crates/core/src/registry.rs` `VERBS` and `SHARED_WORDS`: deliberate
  one-line edits.
- `config.example.toml`: one file for humans; sections are appends.

## Phase 2: cards, rules, tools, skill

### 2.1 Domain cards

Every crate gets a nested card. Claude Code loads a nested `CLAUDE.md` only
when it reads files in that directory. Codex and other agents read nested
`AGENTS.md`.

- [x] **Card files:** for each crate (`ado`, `azure`, `k8s`, `sql`,
  `airflow`, `dd`, `core`, `cli`), add `crates/<crate>/AGENTS.md` (at most
  60 lines / 3 KB) and `crates/<crate>/CLAUDE.md` containing exactly
  `@AGENTS.md`.
- [x] **Domain card sections** (azure covers kv, acr and aks):
  1. **What it is:** one line, plus the service's API docs link.
  2. **Config:** the section and keys, the credential keys (`KEY`, `KEY_env`,
     `KEY_cmd`), and hosts a token may go to.
  3. **Ids:** the format of each resource's `id`, and what the receiving
     verbs accept.
  4. **Where things are:** `lib.rs` is the index, `client.rs` does HTTP and
     auth, `<resource>/mod.rs` holds the resource's shared rows. Name the
     helpers a handler calls (e.g. `Ado::get`, `Ado::query_post`), with one
     line each.
  5. **Fixtures:** its `fixtures/world/http/<domain>.json`, its facts file,
     its world test file.
  6. **Quirks:** the non-obvious things. For ado: the 203 sign-in page, WIQL
     `$top`, `timePrecision`. For airflow: `dry_run` defaults to true, the
     same-origin token rule. For dd: key headers attach at send time, the
     7-day `--for` cap. For sql: the classifier, `panic=unwind`, Oracle's
     `dlopen`. For k8s: the fake kubectl, `pick()` for scopes.
  7. **Never needed:** files an agent working here can skip.
- [x] **The core card** (`crates/core/AGENTS.md`) is the core API cheat
  sheet: every `pub use` in `crates/core/src/lib.rs` with one line on when to
  use it, grouped (registry, Ctx, HTTP, errors, time, credentials, testing).
  Add a test in `crates/cli/src/main.rs`,
  `the_core_card_names_every_public_export`, that parses the `pub use` names
  from `crates/core/src/lib.rs` and fails if one is missing from
  `crates/core/AGENTS.md`.
- [x] **The cli card** says what the gate tests are, how the reference is
  regenerated, and where the world tests live.
- [x] **Verify:** `wc -c crates/*/AGENTS.md` shows every card ≤ 3 KB, and the
  new test passes. Have a fresh subagent read only `crates/dd/AGENTS.md` and
  the core card, then state where a new dd command goes and which helper
  sends a read-only POST. Its answer must be right.

### 2.2 Root context rules

- [x] **Trim `AGENTS.md`** to at most 8 KB: the rules, the conventions across
  domains, and a new **Context rules** section:
  - learn commands from the binary (`search`, `--help`), not from source
  - don't read `PLAN.md`, `docs/plans/`, `docs/trials/` or
    `docs/reference/`
  - grep fixtures by URL
  - add queries to your crate's `search.toml` without reading the others
  - run `scripts/check.sh <crate>` while working and the full checks before
    committing
  - the crate cards exist and load when you work in a crate
- [x] **Mark the history docs.** `PLAN.md` and `docs/plans/*.md` get a first
  line: `> History: the original plan (2026-09-29). For the current design
  read docs/explanation/design.md; for current work read TODO.md.` Nothing
  else in them changes.
- [x] **Verify:** `wc -c AGENTS.md` ≤ 8192, and no file other than docs
  history links to `PLAN.md` as current guidance (`grep -rn PLAN.md --include=*.md .`).

### 2.3 Scripts

Plain shell; no xtask crate.

- [x] **`scripts/check.sh [crate…] [--all]`.** For the named crates it runs:
  - `cargo fmt --all -- --check`
  - `cargo clippy -p agent-cli-<crate> --all-targets -- -D warnings`
  - `cargo test -q -p agent-cli-<crate>`
  - the gate tests: `cargo test -q -p agent-cli --features fixtures`

  It prints only failures and a final line such as `check: ado ok (5 steps)`.
  `--all` runs exactly CI's five checks. It exits non-zero on the first
  failure.
- [x] **`scripts/new-command.sh <domain> <resource> <verb> --effect
  read|write|destructive|reveal|varies`.** It writes a new command from
  `scripts/templates/command.rs`, where the path follows Phase 3's layout:
  - `crates/<crate>/src/<resource>/<verb>.rs`
  - or `…/src/<domain>/<resource>/<verb>.rs` in the azure crate

  It also:
  - adds `mod <verb>;` to that resource's `mod.rs`, creating it if needed
  - adds the const to the domain's `COMMANDS` in `lib.rs`
  - appends two placeholder queries (`text = "TODO …"`) to the crate's
    `search.toml`, which the search gate rejects until they're replaced

  The template holds:
  - an args struct (`--limit` when the verb is `list`)
  - a return struct
  - a handler using `ctx.read` / `ctx.write`
  - the `command!` block
  - a fixture test, plus a dry-run test for write effects

  The generated file **compiles**, and its tests **fail** until they're
  filled in. Unknown domains or verbs exit 2 with the valid choices, and it
  refuses to overwrite an existing file.
- [x] **Verify:**
  - `scripts/new-command.sh dd monitor mute --effect destructive` (or any
    unused path) produces a tree that `cargo build` accepts.
  - `cargo test -p agent-cli-dd` fails only in the new file's tests.
  - The search gate fails on the TODO queries.
  - Then `git checkout . && git clean -fd crates/dd`.
  - `scripts/check.sh sql` passes and prints at most a few lines.
  - `scripts/check.sh --all` passes.

### 2.4 The repo skill

Skill location follows the user's convention (`.agents/skills/<name>/`, as in
`~/dev/ticket-tui`). Claude Code only discovers `.claude/skills/`, so commit a
symlink. Before writing, load the user's skill-authoring standards at
`~/.claude/skills/jacob-create-skill/SKILL.md` and follow them.

- [x] **`.agents/skills/agent-cli-dev/SKILL.md`** (≤ 100 lines):
  - **Frontmatter:** `name`, a trigger-rich `description` (add or change an
    agent-cli command, domain, search query, fixture, or run trials), and
    `paths: ["crates/**", "fixtures/**", "scripts/**", "docs/how-to/**"]`.
  - **Body:** a task router. For each task below, say what to read (at most 4
    files: the crate card, the core card, a sibling command file, the how-to
    section), what to run, and how to verify:
    - new command
    - change a command's args or output
    - new domain
    - search miss
    - fixture or world change
    - agent trial
  - **Reuse, don't copy:** link to `docs/how-to/*.md` sections instead of
    duplicating them.
- [x] **`references/`** only for what the how-tos don't cover: e.g. a
  `checklist.md` with the pre-commit checklist, and `trials.md` with the
  development-trial prompt and the table columns from Phase 0.
- [x] **Link it for Claude Code:** commit the symlink `.claude/skills/agent-cli-dev ->
  ../../.agents/skills/agent-cli-dev`. In `.gitignore`, replace `/.claude/`
  with `/.claude/*`, `!/.claude/skills/` and `!/.claude/settings.json`.
- [x] **Verify:**
  - `git ls-files .claude .agents` shows the skill, the symlink and nothing
    else from `.claude/`.
  - In a fresh Claude Code session in the repo, the skill is listed.
  - A subagent asked "add a command to agent-cli" loads it.

### 2.5 Project settings

- [x] **Deny reads of generated files:** `.claude/settings.json` (committed)
  denies `Read` of `docs/reference/**`, `Cargo.lock` and `target/**`. Check
  the permission rule syntax in Claude Code's settings docs.
- [x] **Code navigation:** check Claude Code's docs for the rust-analyzer
  code-intelligence (LSP) plugin.
  - **If it's a per-user install:** add one line to the skill ("with the
    rust-analyzer plugin, jump to definitions instead of reading whole
    files") and don't commit user config.
  - **If a project setting enables it:** commit that setting.
- [x] **Verify:** in a fresh session, reading `docs/reference/ado.md` is
  denied. `cargo test` is unaffected, since tests read through `std::fs`.

### 2.6 Check Phase 2

- [x] All five checks pass, including the new core-card test.
- [x] `git status` is clean after `UPDATE_DOCS=1 cargo test -p agent-cli
  reference`.

## Phase 3: one file per command

### 3.1 The target layout

```
crates/ado/src/
  lib.rs              the index: DOMAIN (status and doctor may call into client.rs) and COMMANDS; nothing else
  client.rs           HTTP, auth, error mapping, shared request helpers
  markdown.rs         shared helpers keep their own modules
  workitem/
    mod.rs            rows and helpers shared by work-item commands; `mod` lines
    list.rs           WORKITEM_LIST: args, handler, tests for this command
    get.rs  create.rs  update.rs  comment.rs  link.rs
  team/list.rs        (with team/mod.rs)
  repo/  pr/  pipeline/  run/  approval/
crates/azure/src/
  kv/{vault,secret,version}/…   acr/{registry,repo,tag,manifest}/…   aks/cluster/…
```

**Rules:**
- **File path mirrors the command.** `<resource>/<verb>.rs`, with the domain
  as a first directory only in multi-domain crates (azure). Hyphenated
  resources use underscores (`import-error` → `import_error/`, `log-count` →
  `log_count/`).
- **Where shared code goes:**
  - Code shared by one resource's verbs goes in `<resource>/mod.rs`.
  - Code shared across resources goes in a crate-level module
    (`client.rs`, `ids.rs`, …).
  - **No command file imports another command file.**
- **Tests** for a command live in its file (`#[cfg(test)] mod tests`). Test
  helpers shared across a crate go in `src/testing.rs` behind `#[cfg(test)]`.
- **Registration order** in `COMMANDS` stays exactly as it is, since the
  listing order is visible to agents.

### 3.2 Make the layout checkable (core, before the split)

- [x] **Record the source file:** add `source: &'static str` to
  `agent_cli_core::Command`, and have `command!` fill it with `file!()`.
  Nothing prints it.
- [x] **Invariant:** `check_registry` checks that `source` ends with
  `<resource>/<verb>.rs` (or `<domain>/<resource>/<verb>.rs`), hyphens →
  underscores. Behind a flag at first:
  `AGENT_CLI_CHECK_LAYOUT=1` or a separate `check_layout()` the gate calls
  only once Phase 3 is complete. Turn it on permanently in 3.5.
- [x] **File-size test:** `no_source_file_is_too_long` in
  `crates/cli/src/main.rs` walks `crates/*/src/**/*.rs` and fails any file
  over 600 lines. The exception is paths listed in `scripts/large-files.txt`,
  each with its current line count; the test also fails if a listed file
  grows past its recorded count. The list may only shrink. Seed it with
  today's files over 600 lines, then remove each command file as the split
  moves it out.

### 3.3 The split, crate by crate

- [x] **Before you start, record for each crate:**
  - `cargo test -q -p agent-cli-<crate> -- --list 2>/dev/null | grep -c ':
    test$'` (the test count)
  - `target/debug/agent-cli <domain> <resource> <verb> --help` for every
    command, into a before-directory (a loop over `agent-cli <domain>`
    listings)
- [x] **Pilot on `sql`** (6 commands, the smallest) in one worktree:
  - Move each command's const, args, rows, handler and tests into its own
    file.
  - Move shared code as the rules say.
  - `lib.rs` becomes the index.
  - Don't reword code while moving it.

  Then check the pilot:
  - the test count is identical
  - every `--help` is byte-identical
  - the search gate is identical
  - `AGENT_CLI_TEST_DBS=1 cargo test -p agent-cli-sql` passes against the
    compose databases

  **Stop and review the pilot's diff before continuing.**
- [x] **Then the other five crates,** one agent per crate in its own worktree,
  at most two at a time for memory: `k8s`, `azure`, `airflow`, `dd`, `ado`
  (largest last). Give each agent:
  - this section
  - the pilot's commit hash, as the example to follow
  - the crate's card

  Each crate has to meet the same checks as the pilot: identical test count,
  byte-identical `--help`, identical gate numbers.
- [x] **Merge each crate branch** into `main` as it lands. Because Phase 1
  removed the shared files, a merge should touch only `crates/<crate>/**`,
  `scripts/large-files.txt` and `Cargo.lock`. Record any other conflict in
  the Phase 4 notes as a finding.

### 3.4 Update the tools and docs to the layout

- [x] **Scaffold:** `scripts/new-command.sh` now writes into the new layout
  (it was written against it in 2.3; re-run its verification).
- [x] **How-tos:** `docs/how-to/add-a-command.md` walks through the new
  layout. Its worked example (`ado approval list` / `approve`) points at
  `crates/ado/src/approval/list.rs` and `approve.rs`.
  `every_command_line_in_the_docs_parses` still passes.
- [x] **Cards:** update each crate card's "Where things are" section.

### 3.5 Check Phase 3

- [x] Turn the layout invariant on permanently. `check_registry` passes.
- [x] `no_source_file_is_too_long` passes. `scripts/large-files.txt` lists
  only non-command modules (e.g. `core/http.rs`, `ado/markdown.rs`,
  `sql/catalog.rs`), and each has a reason in a comment.
- [x] Every domain `lib.rs` is at most about 120 lines.
- [x] All five checks pass. The overview and every `--help` are
  byte-identical to Phase 0's. `docs/reference/*` needs no regeneration.

## Phase 4: measure again and write it down

- [x] **Re-run D1–D3** exactly as in 0.2: same prompt, same models, fresh
  worktrees from the new `main`. Record the same columns in
  `docs/trials/dev-after-2026-MM-DD.md`, next to the baseline.
- [x] **Success criteria.** If a criterion fails, say so plainly in the doc,
  and add a TODO with the likely cause.
  - [x] Every run passes all five checks. There were no baseline failures
    to fix, so any failure is a regression.
  - [ ] Files changed outside the task's crate are limited to the task's
    fixtures, facts, world test and reference page.
  - [ ] Median tokens per task are at least 25% below the baseline.
  - [x] No run reads `PLAN.md`, `docs/plans/` or the generated reference.
    Check the agents' reports, or ask each agent at the end to list the files
    it read.
- [x] **Explain the design:** update `docs/explanation/design.md` with a short
  section, "Built for small units of work": the layout, the cards, the skill,
  and the before/after numbers.
- [x] **TODO.md:** tick this plan's items; add what the trials found.
- [x] Delete the trial worktrees and branches.
- [x] Add a first line to this file: `> Done on <date>; results in
  docs/trials/dev-after-….md`.

## Acceptance checklist (the whole plan)

- [x] CI's five checks pass. Search gate: 253/253 top-1 and top-5, plus any
  queries added since. Same test count or higher.
- [x] The overview and every command's `--help` are byte-identical to the
  start.
- [ ] Adding a command in an existing domain touches only that crate, its
  fixtures and facts file, its world test (optional) and its reference page.
  This is shown by D1–D3 after the refactor.
- [x] No source file over 600 lines outside `scripts/large-files.txt`. That
  list holds no command files.
- [x] Every crate has a card of at most 3 KB. The core card names every
  public export (tested).
- [x] `AGENTS.md` ≤ 8 KB, with context rules. `PLAN.md` and `docs/plans/` are
  marked as history.
- [x] Two scripts work: `scripts/check.sh` (scoped and `--all`) and
  `scripts/new-command.sh`, whose output compiles and fails until filled in.
- [x] The `agent-cli-dev` skill is in `.agents/skills/`, symlinked for Claude
  Code, and appears in a fresh session.
- [x] `docs/trials/dev-baseline-2026-09-30.md` and `docs/trials/dev-after-….md`
  exist, and the after-run meets Phase 4's criteria or says why not.

## What not to do

- **No new behaviour, flags or commands** outside the D1–D3 trial branches,
  which are never merged. File bugs you notice in `TODO.md` instead of fixing
  them mid-refactor.
- **Don't split the repo or publish crates separately.** The registry-wide
  checks (`check_registry`, the search gate, read-only refusal, the overview
  size) are what keep 1,000 commands coherent.
- **No `CLAUDE.md`/`AGENTS.md` per resource directory.** One card per crate is
  enough.
- **Don't copy the how-tos into the skill.** The skill routes; the docs
  explain.
- **No xtask crate, code generator or proc-macro.** Two shell scripts and a
  template are the ceiling until they measurably hurt.
- **Don't rewrite code while moving it** in Phase 3. A move that changes
  behaviour is a different commit with its own test.
