# agent-cli: conventions for every contributor (human or agent)

agent-cli is one Rust binary whose only users are AI coding agents. Every
command is `agent-cli <domain> <resource> <verb>`. It is built to hold 1,000+
commands, so the rules below are enforced by tests, not by review. `PLAN.md`
is the approved design; `crates/core` is the runtime every domain plugs into.

## Layout

```
crates/core/   agent-cli-core: registry + command! macro, dispatch, discovery
               (overview, listings, help), search, output guard, errors and
               redaction, Ctx (the read/write chokepoint), config, cache,
               process runner, HTTP transport, az tokens, testing helpers
crates/cli/    agent-cli: main() = core::run(DOMAINS), registry-level tests,
               tests/search.toml (labeled search queries), tests/e2e.rs
crates/<name>/ one crate per domain (sql, ado, azure, k8s), each exporting
               `pub const DOMAIN: Domain`
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
- **Dependencies** are the workspace list in `Cargo.toml`. A new one needs a
  one-line reason in the commit message.

## Style

- Edition 2024, `anyhow` at the edges, a typed `Failure` for exit codes.
- Doc comments explain why, not what. No comment restates the code.
- No trait with one implementation, no abstraction for later.
- Mark a deliberate shortcut with a `// ponytail:` comment naming its ceiling.
- Tests sit beside the code (`#[cfg(test)] mod tests`); end-to-end tests live
  in `tests/`. Test names are sentences.

## Adding a command

1. **Args struct** in the domain crate: `#[derive(clap::Args)]`. The first
   line of each field's doc comment is its help text. The main identifier is
   positional (`ado pr get 123`). Lists take `--limit` with default 50. Reuse
   an existing flag name only with the same kind of value; never declare
   `--fields --raw --dry-run --yes --reveal --timeout --output --no-cache` or
   `-h`.
2. **Return struct**: `#[derive(Serialize, JsonSchema)]`, fields in the order
   an agent wants them (id, name or title, state first). This type is the
   `Returns:` line in help and feeds search, so it must be what the command
   really returns.
3. **Handler**: `fn verb(ctx: &Ctx, args: Args) -> anyhow::Result<Ret>`, all
   effects through `ctx.read` / `ctx.write`.
4. **Register it** with the macro:

   ```rust
   command! {
       pub PR_GET = ["ado", "pr", "get"], Read,
       "Show a pull request with its reviewers and votes",
       keywords: ["review", "approved", "who"],
       example: "ado pr get 42",
       run: pr_get,
   }
   ```

   - **Effect:** `Read`; `Write`; `Destructive` for anything hard to undo
     (delete, complete, approve, restart, scale); `Reveal` when it prints a
     secret; `Varies` when the input decides (the handler then sends each
     write through `ctx.write`).
   - **Summary:** imperative, at most 80 characters, no trailing period.
   - **Keywords:** words an agent would use that are not already in the path
     or summary.
   - **Example:** starts with the path, runs as written, and uses `--fields`
     when the command returns a list of objects.
   - **Verb:** one of `VERBS` in `crates/core/src/registry.rs`. Adding a verb
     is a deliberate one-line edit there, only when no existing verb fits.
5. **Add it to the domain's `COMMANDS` slice** (the order is the listing
   order). A new domain also goes into `DOMAINS` in `crates/cli/src/main.rs`,
   the workspace members, and `crates/cli/Cargo.toml`.
6. **Add at least two labeled queries** to `crates/cli/tests/search.toml`,
   written as tasks (`text = "who approved PR 42"`,
   `expect = "ado pr get"`).
7. **Fixture test**: run it with `testing::run(DOMAINS, argv,
   Setup::fake(FakeTransport::answering([...])))` over synthetic or scrubbed
   answers, and assert on stdout, stderr and the exit code.
8. **Dry-run test** for every `Write`, `Destructive` or `Varies` command:
   `testing::assert_dry_run(DOMAINS, argv, answers_for_the_reads)`.
9. **Run the checks**: `cargo fmt --all`, `cargo clippy --workspace
   --all-targets -- -D warnings`, `cargo test --workspace`. The tests include
   the registry invariants (`check_registry`), the search gates (top-1 at
   least 80%, top-5 at least 95%), read-only refusal of every non-read
   command, and the 1,000-command perf gates.

## Checks before any commit

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The sql integration tests need the two compose databases: `scripts/db-up.sh`,
then `AGENT_CLI_TEST_DBS=1 cargo test --workspace`. Without the variable they
skip.
