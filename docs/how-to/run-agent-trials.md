# How to run agent trials

An agent trial measures what agents actually do with agent-cli: fresh
subagents get a task and the tool's path, nothing else, and a wrapper logs
every call they make. Run one when a phase lands, when a domain is added, or
when a change to search, help or errors needs evidence. Every miss becomes a
fix, a test or a labeled search query. The method is design.md's (the api-cli
skill's `references/design.md`), which measured the interface this tool
ports; the last round's results are the baseline in
[docs/trials/2026-09-30.md](../trials/2026-09-30.md).

Prerequisites:

- Claude Code, able to start subagents on Sonnet 5.5 and Haiku 4.5.
- This repository, Rust 1.88 or later and a C compiler.
- A trial directory outside the repository, so agents cannot read the code.
- For SQL tasks, the compose databases: `scripts/db-up.sh`.

## 1. Write the tasks

Write each task as a user would say it, with its answer written down before
the run:

- The answer must be true in the contoso world (see the facts table in
  `fixtures/world/README.md`). A task the world cannot answer measures the
  harness, not the tool.
- Name no command, flag or domain the way the CLI spells it. "Which of my
  work items in the current sprint are not resolved?" is a task; "run `ado
  workitem list --iteration @current`" is a hint.
- Cover at least six tasks per domain under test, some single-domain and some
  that cross domains (the deploy trace, a failed DAG to its pod), and one that
  changes something, to see whether agents respect `--yes` and `--dry-run`.
- Decide what counts as correct, and any threshold a change must meet, before
  the run.

## 2. Build the binary and the logging wrapper

A `cargo test` rebuilds `target/debug/agent-cli` without the fixtures
feature, so the trial gets its own copy. From the repository's root:

```sh
export T=$HOME/agent-trials/2026-10-07
export REPO=$PWD
mkdir -p "$T/runs"
cargo build --features fixtures -p agent-cli
cp target/debug/agent-cli "$T/agent-cli-bin"
```

Replace `$HOME/agent-trials/2026-10-07` with a new directory for this round,
so its log starts empty.

Save this as `$T/wrapper.sh` and make it executable. For each run it writes an
`agent-cli` that sets up the world, runs the copy, and appends one line per
call to `$T/log.tsv`: run id, time, exit code, stdout bytes, stderr bytes and
the arguments:

```sh
#!/bin/sh
# wrapper.sh ID: writes $T/runs/ID/agent-cli, an agent-cli that logs each call
# to $T/log.tsv as run id, time, exit code, stdout bytes, stderr bytes, argv.
set -eu
id=$1
mkdir -p "$T/runs/$id"
cat > "$T/runs/$id/agent-cli" <<EOF
#!/bin/sh
eval "\$('$REPO/scripts/trial-env.sh')"
o=\$(mktemp); e=\$(mktemp)
'$T/agent-cli-bin' "\$@" >"\$o" 2>"\$e"; c=\$?
argv=\$(printf '%s' "\$*" | tr '\t\n' '  ')
printf '%s\t%s\t%s\t%s\t%s\t%s\n' '$id' "\$(date +%s)" "\$c" "\$(wc -c <"\$o")" "\$(wc -c <"\$e")" "\$argv" >>'$T/log.tsv'
cat "\$o"; cat "\$e" >&2; rm -f "\$o" "\$e"; exit \$c
EOF
chmod +x "$T/runs/$id/agent-cli"
echo "$T/runs/$id/agent-cli"
```

`scripts/trial-env.sh` sets `AGENT_CLI_FIXTURES_MATCH=loose`: a request the
world did not record exactly (an agent's own `--since 2d` or `--limit 10`) is
answered with the closest recording of its method and path, and a path never
recorded with a 404, so the agent sees a service rather than the harness. For
SQL tasks, copy `fixtures/world/config.toml` into `$T`, append the compose
connections from the repository's `config.test.toml`, and in `wrapper.sh` add
the line `export AGENT_CLI_CONFIG='$T/config.toml'` after the `eval` line of
the generated script.

Make one wrapper per task and model, named by the task and a last letter for
the model, `s` for Sonnet and `h` for Haiku (step 4 reads the model from that
letter):

```sh
"$T/wrapper.sh" t01s
"$T/wrapper.sh" t01h
```

Try one before handing it out: `"$T/runs/t01s/agent-cli" --help` should print
the overview with `Now: 2026-09-29T12:00Z`, and add a line to `$T/log.tsv`
(delete that line before the run).

## 3. Start the agents

Start the Claude Code session that runs the trial in `$T`, outside the
repository, so no subagent inherits the repository's `CLAUDE.md`. From it,
start one fresh subagent per task and model, in parallel, with its model set
to Sonnet 5.5 or Haiku 4.5, no skill, and this prompt:

```text
You have a command-line tool for Azure DevOps, Azure (Key Vault, Container
Registry, AKS), Kubernetes, SQL, Airflow and Datadog at PATH. It is already
configured. Task: TASK. Use only that tool; don't read its files.
```

Replace `PATH` with the run's wrapper (the path `wrapper.sh` printed) and
`TASK` with the task's text. From each subagent's report, record its answer,
total tokens, tool calls and wall time in the round's results page (step 5).

## 4. Measure

Check each answer against the one written in step 1. Then summarise the log;
this prints calls, stdout bytes, `--fields` use, searches and help or listing
calls per model, and the failing exit codes:

```sh
awk -F'\t' 'NF == 6 {m = substr($1, length($1)); calls[m]++; out[m] += $4; if ($3 != 0) failed[m " exit " $3]++; if ($6 ~ /--fields/) fields[m]++; if ($6 ~ /^search /) search[m]++; if ($6 == "" || $6 ~ /--help/ || $6 ~ /^[a-z0-9-]+( [a-z0-9-]+)?$/) browse[m]++}
END {for (m in calls) printf "%s: %d calls, %d stdout bytes, %d with --fields, %d searches, %d help or listings\n", m, calls[m], out[m], fields[m], search[m], browse[m]; for (k in failed) printf "%s: %d\n", k, failed[k]}' "$T/log.tsv"
```

Per task, count the calls (`cut -f1 "$T/log.tsv" | sort | uniq -c`). Look for
stdout over 12 KB (the guard cut it) and read every line with a non-zero
exit: those are where agents lost time.

## 5. Turn findings into changes

Sort every failing call into one of three kinds, and give each a change that
a test holds. A refusal the agent answered correctly (a destructive command
re-run with `--yes` when the task asked for the change) is the safety model
working, not a finding. A wrong answer with no failed call is one: read that
agent's transcript for where it went astray.

- **The harness answered badly:** the world lacks what a natural call asks
  for, or the harness showed through. Record the exchange (the world's README
  says how) and add the call to `crates/cli/tests/world.rs`.
- **The tool misled the agent:** an error that pointed the wrong way, a hint
  that did not parse, a destructive command suggested for a lookup. Fix it in
  core or the domain, with a test that shows the new message.
- **Search sent the agent astray:** add the searches agents typed, and each
  task as written, to `crates/cli/tests/search.toml` with the command an
  agent should reach first, then tune words until the gate passes.

Record the round in `docs/trials/<date>.md`: the setup, the tasks with their
answers, the per-task table, the findings and what changed. The next round
compares against it, task by task: correctness, calls, tokens and failed
calls. Rebuild and copy the binary again before the next round, so it runs
the fixes.
