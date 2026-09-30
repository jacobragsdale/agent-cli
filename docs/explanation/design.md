# The design of agent-cli

agent-cli's reader is a language model working through a shell. It pays for
every byte it reads, cannot see a screen, has two minutes per call, copies
what it is shown, and believes what it is told. Each rule of the interface
trades something a person would want for something that reader needs, and
each was measured: first by the api-cli trials in design.md (no-context
Sonnet and Haiku agents against Gitea, GitHub, weather.gov and a demo
exchange, 2026-09-28), then by this repository's own trial of all eight
domains ([2026-09-30](../trials/2026-09-30.md)).

## One binary, three words

Every command is `agent-cli <domain> <resource> <verb>`, with the verb from a
closed list of 21. An agent that has used `ado run get` can guess `airflow
run get`, and a test refuses a verb that is not on the list, so the same
action never gets two names. One binary, rather than a CLI per service, means
one discovery surface, one set of flag conventions, and one place to fix a
bug for every domain. Commands are static data (`command!` builds each from
the handler's own argument and return types), which is what lets a single
test hold 1,000 of them to the same rules.

## Search first, browsing cheap

The overview lists domains, not commands, and stays under 1 KB however many
commands exist; the first thing it says is to search. In design.md, Haiku
reached for search first, while Sonnet found commands on familiar APIs with
or without it; listings of more than 40 entries print names only, which took
GitHub's largest listing from 19.8 KB to 5 KB. This round showed the same
split: Haiku searched 13 times and Sonnet 8, while Sonnet made 47 help and
listing calls to Haiku's 26.

Search is a small ranker (BM25F over path, summary, keywords, argument names
and return fields, times the squared share of words matched) plus each
domain's synonyms. Its quality is a number, not a feeling: labeled queries in
each crate's `search.toml` gate every change at top-1 80% and top-5 95%.
This round showed why synonyms belong to their domain. ado's "build" meant
`run`, so "why did the build fail" ranked `airflow run get` first; now a
domain's synonyms count only for its own commands.

## JSON, `--fields` and the guard

Output is compact JSON with nulls and empties dropped: minified JSON is 10 to
36% smaller than indented, and field selection matters more than format (in
design.md's count, 50 GitHub issues were 123k tokens with every field and 2.6k
with six). So every command's help ends with its exact `Returns:` shape and an
example that uses `--fields`, and agents in design.md used `--fields` on their
first data call in almost every transcript. The naive arm there pulled 17 MB
of pull requests where the agent-first arm read 7 KB.

The 12 KB guard is the backstop: it cuts the largest list so what prints is
still valid JSON, saves the whole answer to a file, and says how to narrow
it. It cuts structurally because a text cut produced invalid JSON in
design.md's round 2, and Haiku spent 17 calls recovering. This round no call
reached the guard, over about 104 KB of stdout for 187 calls, and bounded
defaults (`--limit 50`, `--tail 200`, rows that are already narrow) did most
of that: Haiku used `--fields` on only 2 of its 81 calls.

## Errors that name the next step

An agent reads the exit code before the message, so the code says what kind
of step comes next: 2 fix the call, 3 set something up, 4 not found, 5
conflict, 124 timed out, 1 anything else. The message says what went wrong in
the service's own words, and a `hint:` names the command to run, which a test
parses against the registry so a hint can never point at a command that does
not exist.

This round showed that a hint has to lead with the likely intent. clap's
"tip: to pass '--log-id' as a value, use '-- --log-id'" sent an agent the
wrong way; now an unknown flag lists the flags the command has. `ado run 8809`
listed `run cancel` first; now the id's `get` leads and reads come before
changes, because a failed lookup is rarely a request to delete something.

## One door for every effect

Every HTTP request, child process and SQL batch goes through `ctx.read` or
`ctx.write`. `write` is the one place `--dry-run` stops before the first
change, `AGENT_CLI_READ_ONLY` refuses it, and a destructive command without
`--yes` is refused with the exact command to re-run. A handler cannot forget
a check it never makes, and two tests prove it for every command: nothing
that writes reaches the transport under `--dry-run`, and read-only mode
refuses every non-read command before anything is sent. Secrets get the same
treatment: a value lives in a `Secret` that has no serializer, only a
`reveal` command prints one, and a token is attached only to its own
service's hosts. In design.md agents ran `--dry-run` unprompted before
writes. This round, a restart without `--yes` and a secret read without
`--reveal` each cost one refused call, and the refusal said what to add.

## Deadlines

Claude Code's shell gives up at two minutes, so every command has a deadline:
60 seconds by default, 100 for a command that waits, `--timeout` to change
it. Throttle waits and child processes (`az`, `kubectl`) are bounded by the
same deadline, and a child's whole process group is killed at it, so a
device-code prompt cannot hang a call. A wait that runs out exits 124 and
still prints what it waited on.

## Ids are references

The `id` a row prints is exactly what the next command takes, across
domains: `airflow task get` prints the pod as `prod/web/<pod>`, which `k8s
pod logs` takes as it is, and `k8s deployment list` prints the image
reference `acr manifest get` takes. design.md found that agents copy what
they are shown and try positional values first, so an id that is the
reference costs nothing to use. The alternatives cost more:
[cross-domain.md](../plans/cross-domain.md) weighed typed reference objects
(40 to 80 bytes each, and the agent still maps kinds to commands) and `next`
links in the output (dropped by `--fields`, which agents use first). Times
follow the same logic: every printed time is RFC 3339 in UTC, so it pastes
straight into `--since`, and the overview's `Now:` line spares the date
arithmetic most likely to go silently wrong.

## A recorded world for trials

Trials need a world that is reproducible, public and consistent across
services, so the contoso world is synthetic: its Azure DevOps build, image
tag, pods, Airflow run and Datadog alert all tell one story, which is what
makes cross-domain tasks answerable. A cargo feature that no release carries
replays its HTTP recordings, stand-in `az` and `kubectl` scripts serve the
rest, and a frozen clock keeps `--since 1d` meaning the same thing on any
day. Tests match recordings strictly, so a miss names what to record. Trials
match loosely, because this round's eleven fixture misses were all agents
choosing their own windows and limits, and an agent told "fixtures: no
recorded answer" learns about the harness instead of the service.

## Built for small units of work

The registry is meant to reach 1,000 commands, added and changed mostly by
agents, one command at a time. The repository is shaped so that one command
costs an agent a few small files, whatever the registry's size.

- **One file per command.** `ado approval list` is
  `crates/ado/src/approval/list.rs`: its args, rows, handler, `command!` and
  tests. What a resource's verbs share is in its `mod.rs`, what several
  resources share in a crate-level module, and no command file imports
  another. `check_layout` holds every command to its path, and no source file
  passes 600 lines except the shared modules in `scripts/large-files.txt`,
  a list that only shrinks.
- **Each domain owns its files.** Its search queries (`search.toml`), its
  reference page, its world tests and its world facts live apart from the
  other domains', so a routine new command touches its crate, its fixtures
  and facts, its world test and its reference page, and two domains built in
  parallel do not collide.
- **Context in layers.** The root `AGENTS.md` (7 KB) holds the rules and
  says what not to read; each crate has a card of at most 3 KB that loads
  when an agent works there; the core card names every public export (a
  test holds it to `lib.rs`); the `agent-cli-dev` skill routes each kind of
  change to what to read, run and check; `.claude/settings.json` denies
  reading the generated reference. `scripts/new-command.sh` writes a command
  that compiles and fails until filled in, and `scripts/check.sh` runs the
  checks for one crate or all of them.

Fresh agents did three development tasks before and after this layout
([baseline](../trials/dev-baseline-2026-09-30.md),
[after](../trials/dev-after-2026-09-30.md)). After it they read their
crate's card and one or two command files instead of a 19 KB how-to and a
1,000-line resource file, made a quarter fewer tool calls (median 12 to 8)
and touched fewer files outside the crate (median 2 to 1). Tokens fell 29%
on one run per task but 18% on two: each model call carries about 27,000
tokens of fixed context, so what an agent no longer reads matters less than
how many calls it makes.

## What was left out

Some choices are absences. There are no `--json` or `--format` flags: output
is JSON, always. There is no sticky context (`agent-cli use prod`): every
call names its scope or takes the only one, so a transcript reproduces
itself. There are no `next` links in stdout, no mode that changes behaviour
when it detects an agent (design.md cites the bugs that caused elsewhere),
and no one-to-one wrapper of `kubectl` or `az` verbs without a bound or a
join: agents already know those tools, and a command earns its place by what
it adds.

To apply these rules to a new command, see
[How to add a command](../how-to/add-a-command.md); for a new service,
[How to add a domain](../how-to/add-a-domain.md).
