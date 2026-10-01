# agent-cli: the binary and the registry-wide gates

`src/main.rs` is `main() = agent_cli_core::run(DOMAINS)` plus the tests that
hold the whole registry to the rules. `DOMAINS` is the overview's order;
editing it (a new domain) is the only routine change here.

## The gate tests (`src/main.rs`, never loosen them)
- `the_registry_keeps_every_rule`: `check_registry` over every domain
  (paths, verbs, summaries, examples, flag names and kinds, synonyms).
- `search_finds_the_labeled_command`: every `crates/*/search.toml`, top-1 at
  least 80%, top-5 at least 95%. Misses print `want` and the top three:
  `cargo test -p agent-cli --bin agent-cli search_finds -- --nocapture`.
- `read_only_mode_refuses_every_change`: each non-read example under
  `AGENT_CLI_READ_ONLY` is refused before anything is sent.
- `the_overview_fits_with_every_domain_configured`: under 1 KB.
- `the_command_reference_matches_the_registry`: `docs/reference/`, a page
  per domain plus `README.md`; strays fail.
- `every_command_line_in_the_docs_parses`: each `agent-cli …` in `sh` blocks
  and prose of README, AGENTS.md, `docs/` and `fixtures/world/`.
- `the_core_card_names_every_public_export` and
  `the_crate_cards_name_only_what_exists`: the cards match the code.
- `no_source_file_is_too_long`: 600 lines of code, tests aside, but for
  `scripts/large-files.txt`.

## The reference
`UPDATE_DOCS=1 cargo test -p agent-cli reference` rewrites
`docs/reference/*.md` from the registry and removes pages no domain makes.
Never edit those pages by hand.

## World tests (`--features fixtures`)
`tests/common/world.rs` runs the fixtures build against `fixtures/world` as
a trial does (`agent_cli`, `ok`). `tests/world_<domain>.rs` holds what each
domain must answer; `tests/world_cross.rs` the traces that hop domains (the
deploy trace, the failed DAG, the Datadog alert) and what a miss says. Run
one: `cargo test -p agent-cli --features fixtures --test world_dd`.
`tests/ado.rs`, `sql.rs`, `e2e.rs`: end-to-end runs of the real binary.

## Never needed
The domain crates' sources (their cards say where things are),
`docs/reference/`, `docs/plans/`.
