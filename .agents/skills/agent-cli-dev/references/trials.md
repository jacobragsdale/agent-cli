# Development trials

A development trial measures what it costs a fresh agent to change this
repository, the way `docs/how-to/run-agent-trials.md` measures agents using
the CLI. The baseline and its results are in
`docs/trials/dev-baseline-2026-09-30.md`.

## Set up

Each task runs in its own throwaway worktree, never merged:

```sh
git worktree add ../agent-cli-trials/D1 -b trial-D1 main
```

Give every worktree the same start: copy the main checkout's
`target/debug`, leaving out `incremental/` and every `agent_cli*` /
`agent-cli*` artifact, so dependencies are built and the workspace crates
are not. On a machine short of memory, a `.cargo/config.toml` in the
worktrees' parent directory (`[build] jobs = 4`) caps each build without
touching the repository.

## Run

One task at a time, a fresh session each, from the worktree:

```sh
claude -p "$PROMPT" --model claude-sonnet-5-5 --output-format stream-json \
  --verbose --dangerously-skip-permissions > D1.jsonl
```

The prompt is always:

> You are working in the git worktree at `<path>`, a checkout of agent-cli.
> Task: `<task>`. Implement it, make every repo check pass, and commit.
> Don't push.

The tasks: D1 adds `ado pipeline get`, D2 adds `--label` to
`k8s deployment list`, D3 adds the destructive `airflow run cancel`; their
exact wording and what a correct result has are in the baseline doc.

## Record

For each run:

| Column | From |
|---|---|
| Tokens in (all, uncached), out | `scripts/trial_stats.py D1.jsonl` |
| Tool calls, turns, wall time | the same |
| Files read | the same (Read calls and `cat`/`sed`/`head` in Bash) |
| Five checks on the committed tree | `scripts/check.sh --all` in the worktree |
| Files changed, and how many outside the task's crate | `git diff --name-only main...trial-D1` |
| Size | `git diff --shortstat main...trial-D1` |

Then delete the worktrees and branches (`git worktree remove`,
`git branch -D trial-D1`).
