# agent-cli-core: the API a domain uses

`src/lib.rs` re-exports each name below, and `anyhow`, `clap`, `schemars`,
`serde_json` at core's versions.

## Registry
- `command!` makes a `Command` from `fn(&Ctx, Args) -> Result<Row>`;
  `args_of`, `invoke`, `returns_of` are its internals.
- `Domain` (name, summary, commands, synonyms, status, doctor); `Effect`:
  Read, Write, Destructive, Reveal, Varies; `Check`: a doctor line,
  `.denied()` for a 403 (core marks `answered 403` itself). A status
  ending `not set up`, or a denied domain, is left out of search.
- `VERBS`, `SHARED_WORDS`, `SYNONYM_FLAGS`, `GLOBAL_FLAGS`, `BUILTINS`:
  for `check_registry`; `check_layout`: `src/<resource>/<verb>.rs`.
- `run`, `run_with`; `command_help`; `Quality`, `quality`.

## Ctx: every effect goes through it
- `Ctx`: `read(op)`, `write(effect, op)` (--dry-run, --yes, `read_only`),
  `note`, `env`, `section`, `config`, `cache`, `deadline`, `remaining`,
  `long_text` (a `LongText`), `save` (to `--output`); `Op`.
- `enter_scope(noun, name, entry.read_only)` where a domain picks its
  scope; `read_only()`: changes refused there (the entry's key, else the
  section's, which core owns).
- `Globals`, `Setup` (`state_dir`, `secrets`), `DEFAULT_TIMEOUT`.
- `Config` (`[core] include` by name; `KEY_kv = true` from `[core] vault`);
  `pick`: scope flags; `Cache`. No doctor checks: not set up.

## HTTP and processes
- `Request` (`get`, `query` = a POST that only reads, `.json`, `.form`,
  `.bytes`, `.header`, `.auth(Mint)`, `.keep_redirect()`, `.quota_429()`),
  `Response` (`bytes`, `into_bytes`), `Method`, `Body`, `Mint`.
- `Transport`, `Https`: the test seam; the real one.
- `host_under(url, suffix)` before a token goes out; `same_origin` under
  `check_base_url`. `percent_encode`, `form_encode`, `base64`;
  `failure_message`.
- `run_until`, `Output`: a child process by the deadline.
- `on_stop`, `OnStop`: what a signal stops (a DB call) until dropped.

## Diff
`diff(ctx, &DiffArgs, &Rules, left, right)`: two `Side`s (id, row,
`secrets`) to a `Diff` of `Change`s; `Rules::PLAIN`; `scoped`.

## Errors and secrets
- `Failure`: `usage` 2, `setup` 3, `not_found` 4, `conflict` 5, `timed_out`
  124; `.hint(cmd)`, `.with_data(v)`. `Exit`; `status_of`: an HTTP status.
- `Secret` (no Serialize; `expose`), `redact`, `redact_value`, `sensitive_key`.
- `Credential::from_keys(key, value, env, cmd, kv)`, `.resolve(ctx)`,
  `.source()`; `.refresh(ctx)` after a refused sign-in: a changed `KEY_kv`
  value to retry once with, else `None`.

## Time
`When` (`--since`, `--until`; `.utc()`, `.unix()`), `Span` (a length),
`now` (fixtures freeze it), `utc` (a service's time), `utc_time`.

## Testing (`agent_cli_core::testing`)
- `run(DOMAINS, argv, Setup::fake(FakeTransport::answering([Answer::json(&v)])))`
  gives an `Outcome` (`code`, `.json()`); `transport.sent()`: the requests.
- `Setup::with_env`, `with_stdin`, `with_config`, `with_state_dir`,
  `with_secrets`; `assert_dry_run`, `assert_search_quality`,
  `printed_command_problems`, `non_utc_times`, `next_command`.
