# agent-cli-core: the API a domain uses

`src/lib.rs` re-exports all of it; each name below is one `pub use`. Jump to
a definition for details rather than reading whole files.

## Registry
- `command!` makes a `Command` from `fn(&Ctx, Args) -> Result<Row>` (Args:
  `clap::Args`; Row: `Serialize + JsonSchema`). `args_of`, `invoke`,
  `returns_of` are its internals.
- `Domain` (name, summary, commands, synonyms, status, doctor); `Effect`:
  Read, Write, Destructive, Reveal, Varies; `Check`: one doctor line.
- `VERBS`, `SHARED_WORDS`, `SYNONYM_FLAGS`, `GLOBAL_FLAGS`, `BUILTINS`: the
  closed vocabularies `check_registry` enforces. `check_layout`: each
  command in `src/<resource>/<verb>.rs` (its `source`, from `file!()`).
- `run`, `run_with`: the program and its in-process form; `command_help`:
  one command's `--help`. `Quality`, `quality`: search over labeled queries.

## Ctx: every effect goes through it
- `Ctx`: `read(op)`, `write(effect, op)` (where --dry-run, read-only and
  --yes are enforced), `note`, `env`, `section`, `config`, `cache`,
  `deadline`, `remaining`, `long_text` (returns `LongText`), `save` (bytes
  to `--output`; the row then prints). `Op`: what read and write perform.
- `Globals`, `Setup`, `DEFAULT_TIMEOUT`: the run's surroundings.
- `Config`; `pick`: the one rule for scope flags; `Cache`: keyed, with a TTL.
  The binary embeds `config.example.toml`: a domain's doctor that returns no
  checks makes doctor name `agent-cli config example DOMAIN`.

## HTTP and processes
- `Request` (`get`, `query` = a POST that only reads, `.json`, `.form`,
  `.bytes`, `.header`, `.auth(Mint)`, `.keep_redirect()` = a 3xx is the
  answer, as a form sign-in's 302 with its cookie), `Response` (`bytes`
  when not UTF-8, `into_bytes`), `Method`, `Body`, `Mint`.
- `Transport`, `Https`: the seam tests fake, and the real one.
- `host_under(url, suffix)` before any token goes out; `percent_encode`,
  `form_encode`, `base64` (a `Basic` credential); `failure_message`: a
  refusal in the service's own words.
- `run_until`, `Output`: a child process under the deadline.

## Errors and secrets
- `Failure`: `usage` 2, `setup` 3, `not_found` 4, `conflict` 5, `timed_out`
  124; `.hint(cmd)`, `.with_data(v)`. `Exit`; `status_of`: a refusal's HTTP
  status.
- `Secret` (no Serialize; `expose`), `redact`, `redact_value`.
- `Credential::from_keys(key, value, env, cmd)`, `.resolve(ctx)`, `.source()`.

## Time
- `When` (`--since`, `--until`; `.utc()`, `.unix()`), `Span` (a length),
  `now` (fixtures freeze it), `utc` (a service's stamp), `utc_time`.

## Re-exported crates
`anyhow`, `clap`, `schemars`, `serde_json`, at the versions core uses.

## Testing (`agent_cli_core::testing`)
- `run(DOMAINS, argv, Setup::fake(FakeTransport::answering([Answer::json(&v)])))`
  gives an `Outcome` (`code`, `.json()`); `transport.sent()` is what went
  out (with its `headers`).
- `Setup::with_env`, `with_stdin`, `with_config`, `read_only`.
- `assert_dry_run`, `assert_read_only_refuses`, `assert_search_quality`,
  `printed_command_problems`, `non_utc_times`, `next_command` (the argv a
  `[next: agent-cli …]` note names; world tests' `follow` walks them).
