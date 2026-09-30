# agent-cli: conventions for every contributor (human or agent)

agent-cli is one Rust binary whose only users are AI coding agents. Every
command is `agent-cli <domain> <resource> <verb>`. It is built to hold 1,000+
commands, so the rules below are enforced by tests, not by review. `PLAN.md`
is the approved design; `crates/core` is the runtime every domain plugs into.
This file is the contract; the how-to guides in `docs/how-to/` are the
procedures.

## Layout

```
crates/core/   agent-cli-core: registry + command! macro, dispatch, discovery
               (overview, listings, help), search, output guard, errors and
               redaction, Ctx (the read/write chokepoint), config, cache,
               process runner, HTTP transport, az tokens, testing helpers
crates/cli/    agent-cli: main() = core::run(DOMAINS), registry-level tests
               (the command reference, the docs' command lines, the search
               gate over every crate's search.toml), tests/world.rs
crates/<name>/ one crate per domain group (ado, sql, azure, k8s, airflow, dd),
               exporting one `pub const Domain` per domain: DOMAIN, K8S, and
               KV, ACR, AKS from crates/azure; search.toml holds its labeled
               search queries
docs/          tutorial.md, how-to/, reference/ (generated, a page per domain),
               explanation/, trials/ (agent trial results), plans/
fixtures/world the recorded contoso world agent trials run against
scripts/fake   az, kubectl, kubelogin stand-ins (tests and trials)
```

## Rules

- **Public repo.** No real organization, project, subscription, vault,
  registry, cluster or server names anywhere: code, fixtures, docs, commit
  messages. Use `contoso`-style placeholders. Fixtures are synthetic or
  scrubbed. `config.toml` is never committed.
- **Every effect goes through `Ctx`.** HTTP requests, child processes and SQL
  run only via `ctx.read(op)` or `ctx.write(effect, op)`. `write` is where
  `--dry-run`, `AGENT_CLI_READ_ONLY` and `--yes` are enforced; a handler never
  checks those flags itself. A POST that only reads uses `Request::query`.
- **stdout is data only.** Handlers return values; core prints them. Notes
  such as `[50 of 312; --limit N]` go through `ctx.note`.
- **Secrets.** Credentials and secret values live in `Secret` (no `Serialize`,
  `[redacted]` when printed). Check `host_under(url, suffix)` before a token is
  attached to any URL, `nextLink`s included. No list or row type has a field
  that can hold a secret value; only `Reveal` commands return one.
- **Errors carry the next step.** Return `Failure` with the right exit code
  (2 usage, 3 setup, 4 not found, 5 conflict, 124 timed out) and a `hint`
  naming the command to run. Anything else is exit 1.
- **Deadlines.** Nothing waits past `ctx.deadline()`; HTTP and processes
  already use it. Anything else that blocks (a SQL driver) takes
  `ctx.remaining()`.
- **Environment.** Read variables with `ctx.env(name)`, never `std::env`:
  tests set them with `Setup::with_env` (edition 2024 makes `set_var`
  unsafe, and the workspace forbids unsafe code).
- **Dependencies** are the workspace list in `Cargo.toml`. A new one needs a
  one-line reason in the commit message.

## Conventions across domains

These make one domain's output the next domain's input. `check_registry` and
`testing::run` enforce the parts marked (tested); `docs/plans/cross-domain.md`
has the reasoning.

- **Time windows.** A flag that takes a point in time is `--since` or
  `--until` and has type `agent_cli_core::When` (tested both ways): `15m`,
  `2h`, `7d`, `1w`, `now-15m`, a date or RFC 3339, resolved to UTC at parse.
  `--until` defaults to now. Send it to the service with `when.utc()` (or
  `.unix()`); filter client-side on `when.0`. A future window such as
  `--expires-within` or a length such as dd's `--for` is `agent_cli_core::Span`
  and never named since/until. Read the clock with `agent_cli_core::now()`
  (fixture trials freeze it).
- **Printed times** are RFC 3339 UTC in whole seconds ending in `Z` (tested
  on every fixture run): pass a service's stamp through `agent_cli_core::utc`,
  an `OffsetDateTime` through `utc_time`. Log lines keep theirs. Only data
  passed through untouched (`sql query run`) is exempt.
- **The id is the ref.** Every row whose resource has a `get` (or another
  one-object verb) carries `id`: exactly what that verb takes, with no other
  flag. Formats: ado `8812` (also `#8812`, `AB#8812`, the web URL); k8s
  `cluster/namespace/name`; kv `vault/name` (and `/version`); acr
  `loginserver/repo` and `loginserver/repo:tag`; sql `schema.name`. The
  receiving verb takes the id, the pieces as flags (`--cluster`,
  `--namespace`, `--vault`, `--registry`), and a web URL or URI where there
  is one (parsed, never fetched; one naming an unconfigured org or host is
  exit 2). A ref and a flag that disagree is exit 2. A field naming another
  domain's thing holds that thing's id (`secret_refs[].kv`, `images[].image`,
  `aks cluster list`'s `k8s_scope`).
- **Canonical flags** (tested): `--since --until --limit --tail --cluster
  --namespace --conn`; `SYNONYM_FLAGS` in `registry.rs` lists the refused
  spellings (`--from`, `--count`, `--ns`, `--context` …). A list takes
  `--limit` (int, default 50) unless it takes no arguments at all (config-only
  lists); `logs` takes `--tail` and never `--follow`. A flag naming a person
  (`--assignee`, `--author` …) says `@me` works, and `@me` is the
  authenticated user of that domain.
- **Long text** (tested): an argument holding prose, Markdown, SQL or JSON
  (a description, a comment's text, a query, a conf) reads stdin when its
  value is `-`, and a sibling `--NAME-file PATH` (a `PathBuf`) reads it from
  a file. Read both with `ctx.long_text(name, value, file, limit)`: it
  refuses both at once, empty text from a pipe or file, and anything past
  `limit` (comments keep ticket-tui's 64 KiB, and their help says so). Only
  one argument per command can be `-`. Not `@path`: `@` already means `@me`.
- **Scope defaults.** A scope flag (`--cluster`, `--conn`, an instance or
  site) defaults to the only configured one, else exit 2 naming them:
  `agent_cli_core::pick`.
- **Printed command lines parse** (tested on every fixture run): each
  `agent-cli …` in a hint or note resolves against the registry. Write
  placeholders in capitals (`ID`, `NAME`, `PATTERN`); end the command where
  prose resumes (`, then`, ` (`, two spaces, a backtick).
- **An answer with a failing exit.** When the result is worth reading even
  though the command failed (a wait that ended badly, a PR whose links did not
  land), return `Failure::…(…).with_data(value)`: core prints `value` on stdout
  like a success, then the error, and exits with the failure's code. `ado run
  wait` prints the run for exit 1 and 124.
- **Default timeouts.** A command that waits declares `timeout: 100,` in
  `command!` (help shows it); `--timeout` still wins. Everything else keeps
  core's 60 s.
- **Credentials** in config are `KEY` (the file; discouraged), `KEY_env` or
  `KEY_cmd`: keep the three keys in the section and build
  `Credential::from_keys("token", value, env, cmd)?`; `resolve(ctx)` reads the
  variable through `ctx.env` or runs the command under the deadline, trailing
  newline stripped, as a `Secret`. `source()` names it for doctor, never its
  value. A token goes only to its own hosts (`host_under`); a header carrying
  one is masked by name in plans and errors if it is `Authorization`, `*-key`,
  `*_key` or `*token*`.
- **Errors from services.** Core turns a non-2xx into a `Failure` with the
  status (`agent_cli_core::status_of`), the exit code, and the service's own
  words (ARM, ADO, ACR, FastAPI `detail`, Datadog `errors`). Throttles honour
  `Retry-After`, `X-RateLimit-Reset` and ARM's quota clock, within the
  deadline.

The deploy trace (image tag, then the build on that tag, then its PR and
work items) and the failed-DAG trace are in `fixtures/world/README.md`, which
answers both.

## Style

- Edition 2024, `anyhow` at the edges, a typed `Failure` for exit codes.
- Doc comments explain why, not what. No comment restates the code.
- No trait with one implementation, no abstraction for later.
- Mark a deliberate shortcut with a `// ponytail:` comment naming its ceiling.
- Tests sit beside the code (`#[cfg(test)] mod tests`); end-to-end tests live
  in `tests/`. Test names are sentences.

## Adding commands, domains and trials

- A command: `docs/how-to/add-a-command.md`, step by step with a worked
  example (args, return type, handler, `command!`, `COMMANDS`, labeled
  queries, fixture and dry-run tests, the world, the reference).
- A domain: `docs/how-to/add-a-domain.md`, and first whether it earns one.
- An agent trial: `docs/how-to/run-agent-trials.md`. Every miss becomes a
  fix, a test or a labeled query.

## Checks before any commit

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features agent-cli/fixtures -- -D warnings
cargo test --workspace
cargo test --workspace --features agent-cli/fixtures
```

They include the registry invariants (`check_registry`), the search gates
(top-1 at least 80%, top-5 at least 95%), read-only refusal of every non-read
command, the overview budget with every domain configured, the 1,000-command
perf gates, and two docs tests: `docs/reference/` must match the registry
(`UPDATE_DOCS=1 cargo test -p agent-cli reference` rewrites it), and
every `agent-cli …` in the docs' `sh` blocks and prose must parse.

The `fixtures` feature (off by default, never in a release) compiles in the
HTTP replayer: with `AGENT_CLI_FIXTURES=<dir>` every request is answered from
`<dir>/http/*.json`, strictly unless `AGENT_CLI_FIXTURES_MATCH=loose`.
`eval "$(scripts/trial-env.sh)"` sets up a shell to run the fixtures build
against `fixtures/world` (loosely, as trials do); its README says how to
extend it.

The sql integration tests need the two compose databases: `scripts/db-up.sh`,
then `AGENT_CLI_TEST_DBS=1 cargo test --workspace`. Without the variable they
skip.
