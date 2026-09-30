# Development trial baseline, 2026-09-30

What it cost a fresh agent to change this repository before the refactor in
`REFACTOR_PLAN.md`: three development tasks, each run once by Sonnet 5.5 in
a throwaway worktree of commit `ca791db`. Phase 4 repeats them after the
refactor and compares.

## Setup

| Item | Value |
|---|---|
| Tree | `ca791db` (the plan's commit; the code is `88156b8`'s) |
| Agent | a fresh `claude -p` session per task, Sonnet 5.5, stream-json log, from the worktree |
| Prompt | "You are working in the git worktree at `<path>`, a checkout of agent-cli. Task: `<task>`. Implement it, make every repo check pass, and commit. Don't push." |
| Build | each worktree's `target/debug` seeded with the main checkout's dependencies (no workspace crates, no incremental cache); `jobs = 4` from a `.cargo/config.toml` in the worktrees' parent |
| Plugins | the user's rust-analyzer plugin off (`--settings`): with three servers at 1.5 GB each the machine fell under earlyoom's threshold, which killed every rustc |
| Order | one task at a time |

A first attempt ran D1 and D2 together with the plugin on. Both agents spent
their turns retrying builds that earlyoom killed, so the runs were stopped
and are not counted. D1 and D2 were then run once with worktrees that still
held that attempt's build output, and again on clean seeds; the table has
the clean runs (the others: D1 347k tokens, 9 calls; D2 385k, 10 calls).

## Tasks

| Id | Task (as given to the agent) | What a correct result has |
|---|---|---|
| D1 | Add `ado pipeline get <id>`: one pipeline definition by id or name, with its id, name, folder, repository, default branch, YAML path, queue status and web URL | A Read command, args, return struct, handler, fixture test, 2+ search queries, a world recording, and the reference regenerated |
| D2 | Add a `--label key=value` filter (repeatable) to `k8s deployment list`, passed to kubectl as a label selector | A flag on an existing command, a fixture test proving the selector reaches kubectl, a search query, the reference regenerated |
| D3 | Add `airflow run cancel <id>`: mark a queued or running DAG run failed (Airflow 3: `PATCH /api/v2/dags/{dag_id}/dagRuns/{dag_run_id}` with state `failed`). It is destructive | A Destructive command, a dry-run test, read-only refusal, a fixture test, search queries, the reference regenerated |

## Results

Tokens in counts every input token, cached or not; most are the same
context re-read each turn.

| Id | Tokens in (uncached) | Tokens out | Tool calls | Wall | Five checks | Files changed | Outside the crate | Size |
|---|---|---|---|---|---|---|---|---|
| D1 | 350,681 (25,985) | 4,742 | 12 | 86 s | pass | 6 | 4 | +113 −3 |
| D2 | 489,471 (12,940) | 4,674 | 13 | 131 s | pass | 3 | 2 | +29 −5 |
| D3 | 467,387 (37,542) | 3,906 | 12 | 86 s | pass | 4 | 2 | +106 −3 |
| **Median** | **467,387** | **4,674** | **12** | **86 s** | | **4** | **2** | |

Files outside the task's crate:

- D1: `crates/cli/tests/search.toml`, `crates/cli/tests/world.rs`,
  `docs/reference/commands.md`, `fixtures/world/http/ado.json`.
- D2: `docs/reference/commands.md`, `scripts/fake/kubectl` (its deployments
  gained labels to select on).
- D3: `crates/cli/tests/search.toml`, `docs/reference/commands.md`.

Against the "correct result" column: D1 has everything. D2 extended the
existing deployment-list test rather than adding one, and added no search
query. D3 has everything but a world recording, which it judged unneeded,
and it added a guard nobody asked for: a run that already finished is
refused with exit 5.

## Where the reading went

Every agent opened `docs/how-to/add-a-command.md` first (19 KB, the whole
file for D1 and D3, 80 lines for D2), then its domain's big resource file:

- **D1** read `crates/ado/src/pipeline.rs` (1,628 lines) in three slices
  (345 lines), `client.rs` around `pipeline_id`, and one 32-line slice of
  `fixtures/world/http/ado.json` found by grep. It found the search queries
  and the world test by grep in the shared `crates/cli/tests/` files.
- **D2** read slices of `crates/k8s/src/pod.rs` (for `pod list --label`,
  the pattern to copy) and `objects.rs` (the command), then the fake
  kubectl's head and its deployments, which it changed.
- **D3** ran `cat run.rs`: all 922 lines of `crates/airflow/src/run.rs`,
  then slices of it again and `grep`s of `client.rs` for `change` and
  `run_path`, and a slice of the shared `search.toml`.

None read `PLAN.md`, `docs/plans/` or the generated reference; each
regenerated the reference with the test. The context an agent carries
before reading anything (the system prompt, the user's and the repo's
`CLAUDE.md`, the skill list) is about 25k tokens a turn, so turns drive the
total more than reading does.

## Repository shape

`scripts/context-stats.sh` at `ca791db`:

```text
## Largest source files (lines)
   1977 crates/ado/src/workitem.rs
   1782 crates/ado/src/pr.rs
   1628 crates/ado/src/pipeline.rs
   1342 crates/azure/src/acr.rs
   1288 crates/azure/src/kv.rs
   1123 crates/ado/src/client.rs
   1071 crates/airflow/src/task.rs
   1048 crates/airflow/src/client.rs
   1011 crates/sql/src/catalog.rs
   1003 crates/k8s/src/pod.rs
    999 crates/ado/src/markdown.rs
    939 crates/core/src/http.rs
    922 crates/airflow/src/run.rs
    885 crates/k8s/src/objects.rs
    860 crates/dd/src/client.rs

## Lines per crate (src and tests)
   9537 crates/core
   7839 crates/ado
   5607 crates/sql
   4481 crates/dd
   4272 crates/airflow
   4028 crates/azure
   2665 crates/k8s
   1031 crates/cli

## Files touched outside the named crate, last 20 commits
outside/total crate   commit subject
    2/2 -       ca791db A refactor plan for small units of work: per-domain files, crate cards
    4/4 ado     88156b8 Search reads a version as a tag and "how many" as a list, so "v1.4.2 b
  10/14 ado     c9dae27 The old CLIs' list filters: ado pr list --vote, run list --requested-b
    1/4 sql     888263b sql query bench takes --max-rows as query run does: each read keeps th
  16/18 sql     fd77b48 Long text reads stdin for - and a file for --NAME-file: work item and 
    3/3 -       7632a7a AGENTS.md keeps the contract and points at the how-tos, PLAN's phase t
    7/7 -       eeb16b2 The docs follow Diátaxis: a README, a tutorial on the contoso world, 
    4/4 -       f74ed99 The docs check reads sh blocks and the prose around them, and a synony
    4/4 -       c1a9953 docs/reference/commands.md is generated from the registry, and every c
  10/10 -       cd583ae Search scopes each domain's synonyms to its own commands and ranks cha
    2/2 -       66f56f0 Usage errors drop clap's tips: an unknown flag names the close ones an
    5/5 -       11e6ceb Trials match fixtures loosely: a miss gets the closest recording of it
    5/5 -       e1666ef The contoso world answers Datadog: the api alert at the v1.4.2 rollout
   7/15 dd      01383cd The dd domain reads Datadog's REST API: logs, metrics, monitors, downt
    0/1 airflow 99a7984 Airflow run create words a duplicate run plainly, as a live Airflow 3.
    6/6 airflow 0f9abf8 The contoso world has an Airflow whose failed nightly run leads to its
   6/12 airflow c4106d5 The airflow domain reads and runs Apache Airflow 3 DAGs, runs, task in
    0/2 core    0ac0c16 Core shares the word task between ado and airflow and exports redact_v
    4/5 k8s     0bfa0a4 AGENTS.md states the cross-domain conventions and the deploy trace; TO
    9/9 -       df78be0 A fixtures build replays a recorded contoso world for agent trials
```
