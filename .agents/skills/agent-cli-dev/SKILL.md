---
name: agent-cli-dev
description: "Add or change agent-cli commands, domains, search queries, fixtures or trials. Use whenever work touches crates/, fixtures/, scripts/ or docs/how-to/ in this repo, even if the user doesn't say agent-cli."
paths: ["crates/**", "fixtures/**", "scripts/**", "docs/how-to/**"]
---

# agent-cli development

Routes a change to the few files it needs. The how-tos in `docs/how-to/`
explain each step; this skill says what to read, what to run and how to
check, so no task needs a tour of the repository.

## Always

- Read the crate's card, `crates/<crate>/AGENTS.md`, and for the core API
  `crates/core/AGENTS.md`. They name the helpers; read other code only when
  a card points you there.
- Learn existing commands from the binary, not the source:
  `cargo build -q -p agent-cli`, then `target/debug/agent-cli <domain>` and
  `target/debug/agent-cli <domain> <resource> <verb> --help`.
- Never read `docs/plans/` (plans for work in progress), `docs/trials/`,
  `docs/reference/`, `Cargo.lock`, or a whole `fixtures/world/http/*.json`:
  grep it by URL.
- With the rust-analyzer plugin, jump to definitions instead of reading
  whole files.
- Run `scripts/check.sh <crate>` while working and `scripts/check.sh --all`
  before committing; `references/checklist.md` is the pre-commit list.

## Task router

**New command.** Read the crate card, the core card, one sibling command's
file, and `docs/how-to/add-a-command.md` (its first section lists the files
a command touches).
1. `scripts/new-command.sh DOMAIN RESOURCE VERB --effect read|write|destructive|reveal|varies`;
   a wrong domain or verb prints the valid ones.
2. Fill every TODO in the new file, send requests through the domain's
   client (the card names it), and replace the two TODO queries it appended
   to the crate's `search.toml`.
3. Move its line in `lib.rs` to its place in the listing.
4. When a trial could reach it, record the world's answer (how-to §9).
5. `UPDATE_DOCS=1 cargo test -p agent-cli reference`.

Check: `scripts/check.sh <crate>` passes, and `git status` shows only the
crate, `fixtures/world/http/<domain>.json`, `fixtures/world/facts/<domain>.md`,
`crates/cli/tests/world_<domain>.rs` and `docs/reference/<domain>.md`.

**Change a command's args or output.** Read its file, the crate card, and
the Conventions table in `docs/how-to/add-a-command.md`. Change it, extend
its fixture test, regenerate the reference. Check: `scripts/check.sh
<crate>`, and `--help` shows the change.

**New domain.** Read `docs/how-to/add-a-domain.md` (first: does it earn
one?), `crates/dd/AGENTS.md` as the model, and the core card. Check:
`scripts/check.sh --all`, including the overview's 1 KB budget.

**Search miss.** Read the crate's `search.toml` and the expected command's
`--help`. Add the query as the agent typed it, then run
`cargo test -p agent-cli --bin agent-cli search_finds -- --nocapture`: each
miss prints `want` and the top three `got`. Fix words (summary, keywords,
field names, the domain's `synonyms`) before the ranker; never delete a
query. Check: the gate's counts do not drop for any other query.

**Fixture or world change.** Read "Extend it" in `fixtures/world/README.md`,
the domain's `fixtures/world/facts/<domain>.md`, and the command's fixture
test. `cargo build -q --features fixtures -p agent-cli`,
`eval "$(scripts/trial-env.sh)"`, then run the command with
`env -u AGENT_CLI_FIXTURES_MATCH` to see the missing recording. Check:
`cargo test -p agent-cli --features fixtures --test world_<domain>`.

**Agent trial.** For how agents use the CLI, follow
`docs/how-to/run-agent-trials.md`. For development tasks (an agent changing
this repository), read `references/trials.md`.

## Example

"Add `dd monitor update`": read `crates/dd/AGENTS.md` and the core card, run
`scripts/new-command.sh dd monitor update --effect write`, fill in
`crates/dd/src/monitor/update.rs` using `Dd::load(ctx)?` and `dd.change(…)`,
write two real queries in `crates/dd/search.toml`, record the PUT in
`fixtures/world/http/dd.json` if trials could use it, regenerate the
reference, and `scripts/check.sh dd`.

## Bundled resources

- `references/checklist.md`: **read** before every commit.
- `references/trials.md`: **read** when running development trials.
- `scripts/trial_stats.py`: **run** on a trial's stream-json log for its
  tokens, tool calls, wall time and the files it read.
