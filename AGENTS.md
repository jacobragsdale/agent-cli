# agent-cli: conventions for every contributor (human or agent)

agent-cli is one Rust binary whose only users are AI coding agents. Every
command is `agent-cli <domain> <resource> <verb>`. It is built to hold 1,000+
commands, so the rules below are enforced by tests, not by review. This file
is the contract; `docs/how-to/` holds the procedures and
`docs/explanation/design.md` the reasons.

## Layout

```
crates/core/    the runtime every domain plugs into: registry, command!, Ctx
crates/cli/     main() = core::run(DOMAINS), and the registry-wide gate tests
crates/<name>/  one crate per domain group (ado, sql, azure = kv acr aks,
                k8s, airflow, dd): its commands, tests and search.toml
docs/           tutorial, how-to/, explanation/, reference/ (generated)
fixtures/world  the recorded contoso world agent trials run against
scripts/        check.sh, new-command.sh, trial-env.sh, fake az/kubectl
```

Every crate has a card, `crates/<crate>/AGENTS.md` (loaded as `CLAUDE.md`
when you work there): config, ids, where things are, fixtures, quirks.
`crates/core/AGENTS.md` is the core API.

## Context rules

- Learn commands from the binary, the way agents use it:
  `cargo build -p agent-cli`, then `target/debug/agent-cli`,
  `agent-cli search "<words>"`, `agent-cli <domain>` and
  `agent-cli <domain> <resource> <verb> --help`. Not from source.
- Don't read `docs/plans/`, `docs/trials/` or `docs/reference/`: plans for
  work in progress, results and generated help.
- Grep fixtures by URL (`grep -n 'build/definitions' fixtures/world/http/ado.json`);
  never read a whole `http/*.json`, nor `Cargo.lock`.
- Add queries to your crate's `search.toml` without reading the others.
- Run `scripts/check.sh <crate>` while working and `scripts/check.sh --all`
  (CI's five checks) before committing.

## Rules

- **Public repo.** No real organization, project, subscription, vault,
  registry, cluster or server names anywhere: code, fixtures, docs, commit
  messages. Use `contoso`-style placeholders. `config.toml` is never
  committed.
- **Every effect goes through `Ctx`.** HTTP, child processes and SQL run
  only via `ctx.read(op)` or `ctx.write(effect, op)`, where `--dry-run`,
  `AGENT_CLI_READ_ONLY` and `--yes` are enforced; a handler never checks
  those flags. A POST that only reads uses `Request::query`.
- **stdout is data only.** Handlers return values; notes such as
  `[50 of 312; --limit N]` go through `ctx.note`.
- **Secrets** live in `Secret` (no `Serialize`). Check `host_under(url,
  suffix)` before a token goes to any URL, `nextLink`s included. No row type
  has a field that can hold a secret; only `Reveal` commands return one.
- **Errors carry the next step:** `Failure` with its exit code (2 usage, 3
  setup, 4 not found, 5 conflict, 124 timed out) and a `hint` naming the
  command to run. Anything else is exit 1.
- **Deadlines.** Nothing waits past `ctx.deadline()`; a blocking call outside
  HTTP and processes takes `ctx.remaining()`.
- **Environment** through `ctx.env(name)`, never `std::env` (tests use
  `Setup::with_env`; the workspace forbids unsafe code).
- **Dependencies** come from the workspace list; a new one needs a one-line
  reason in the commit message.

## Conventions across domains

They make one domain's output the next domain's input; (tested) marks what
`check_registry` or `testing::run` enforce.

- **Time windows** (tested): a point in time is `--since` or `--until` of
  type `When` (`15m`, `2h`, `7d`, `now-15m`, a date, RFC 3339; UTC at parse).
  Send `when.utc()` or `.unix()`, filter on `when.0`. A future window or a
  length (`--expires-within`, dd's `--for`) is a `Span`. The clock is
  `agent_cli_core::now()`.
- **Printed times** (tested): RFC 3339 UTC, whole seconds, `Z`, through
  `utc` (a service's stamp) or `utc_time`. Log lines keep theirs.
- **The id is the ref.** A row whose resource has a `get` (or another
  one-object verb) carries `id`, exactly what that verb takes. The receiving
  verb also takes the pieces as flags and the service's web URL (parsed,
  never fetched; an unconfigured org or host is exit 2). A ref and a flag
  that disagree is exit 2. A field naming another domain's thing holds that
  thing's id (`secret_refs[].kv`, `images[].image`). Formats are in each
  crate's card.
- **Canonical flags** (tested): `--since --until --limit --tail --cluster
  --namespace --conn`; `SYNONYM_FLAGS` refuses `--from`, `--count`, `--ns`,
  `--context` …. A list takes `--limit` (default 50) unless it takes no
  arguments; `logs` takes `--tail`, never `--follow`. A flag naming a person
  says `@me` works.
- **Long text** (tested): a description, comment, SQL or JSON argument reads
  stdin for `-` and has a sibling `--NAME-file PATH`; read both with
  `ctx.long_text(name, value, file, limit)`. One `-` per command.
- **Scope defaults.** A scope flag defaults to the only configured one, else
  exit 2 naming them: `agent_cli_core::pick`.
- **Printed command lines parse** (tested): every `agent-cli …` in a hint or
  note resolves. Placeholders in capitals (`ID`, `NAME`); end the command
  where prose resumes (`, then`, ` (`, two spaces, a backtick).
- **An answer with a failing exit:** `Failure::…(…).with_data(value)` prints
  `value` like a success, then the error, with the failure's code.
- **Next steps.** A field is not a signpost; a note naming the command is.
  A one-object verb whose answer exits 0 but needs attention (a failed
  run, an alerting monitor, a restarted container) prints one
  `[next: agent-cli …]` note built from its row; a failure puts its next
  command in the hint. Lists print none, but `ado test list`. World tests
  walk them with `follow`.
- **Frames to files.** An `at` from a stack frame is what `ado file get`
  takes. ado prints the repository path; others print the frame's path as
  the service did, and `file get` resolves it by unique suffix.
- **Default timeouts.** A command that waits declares `timeout: 100,` in
  `command!`; everything else keeps core's 60 s.
- **Credentials** are three keys, `KEY`, `KEY_env`, `KEY_cmd`, built with
  `Credential::from_keys` and resolved only when a request goes out;
  `source()` names it for doctor, never its value.
- **Errors from services** become a `Failure` with the status and the
  service's own words in core; throttles honour `Retry-After` and
  `X-RateLimit-Reset` within the deadline.

The deploy trace and the failed-DAG trace are in `fixtures/world/README.md`.

## Style

- Edition 2024, `anyhow` at the edges, a typed `Failure` for exit codes.
- Doc comments explain why, not what. No comment restates the code.
- No trait with one implementation, no abstraction for later.
- Mark a deliberate shortcut with a `// ponytail:` comment naming its ceiling.
- Tests sit beside the code; end-to-end tests in `tests/`. Test names are
  sentences.

## Adding commands, domains and trials

- Any change: the `agent-cli-dev` skill (`.agents/skills/agent-cli-dev/`)
  says what to read, run and check for each kind.
- A command: `scripts/new-command.sh`, then `docs/how-to/add-a-command.md`.
- A domain: `docs/how-to/add-a-domain.md`, and first whether it earns one.
- An agent trial: `docs/how-to/run-agent-trials.md`. Every miss becomes a
  fix, a test or a labeled query.

## Checks before any commit

`scripts/check.sh --all` runs CI's five:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features agent-cli/fixtures -- -D warnings
cargo test --workspace
cargo test --workspace --features agent-cli/fixtures
```

They include `check_registry`, the search gate (top-1 80%, top-5 95%),
read-only refusal, the overview budget, the 1,000-command perf gates, the
reference (`UPDATE_DOCS=1 cargo test -p agent-cli reference` rewrites it)
and the docs' command lines. The `fixtures` feature (never in a release)
replays `fixtures/world`; `eval "$(scripts/trial-env.sh)"` sets up a shell
for it. The sql database tests need `scripts/db-up.sh` and
`AGENT_CLI_TEST_DBS=1`. The live ado suite (`crates/cli/tests/live_ado.rs`)
needs `AGENT_CLI_TEST_ADO=1` and a sandbox project seeded by
`scripts/ado-sandbox.py`; `AGENT_CLI_TEST_ADO_CONFIG` adds a second one.
