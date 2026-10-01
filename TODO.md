# TODO

Open work only. What is built is in the code, the crate cards and
`docs/explanation/design.md`.

## Live runs

- [ ] Work through `docs/first-live-run.md`: no domain but sql has run against its real service
- [ ] Then the one-line global CLAUDE.md note that agent-cli exists (the README gives the line)

## Phase 5: consolidate

- [ ] After the live runs, the TUIs depend on agent-cli's domain crates and delete their copies and their CLIs; each TUI's tests pass on the shared crates
- Not ported, on purpose: ticket-tui `sync` (no local database), `status` (a TUI status-bar line) and `agent launch|prompt|list` (Herdr orchestration; `ado workitem get` gives an agent the content); az-tui `setup --write` (agents get `aks cluster list`, and the README shows the TOML); sql-bench's replay flags and `--format table|csv` (output is JSON only)

## Trials

- [ ] Development trials: tokens fell 29% on one run per task but 18% on two, short of the 25% target (`docs/trials/dev-after-2026-09-30.md`). Each call carries about 27k tokens of fixed context, so cut calls: a scaffold that fills in more of a routine command, a check that builds once for both feature sets. Next round: three runs per task

## Small follow-ups

- [ ] Config env overrides guess types for keys missing from the file ("123" becomes an int)
- [ ] sql `object list` `modified` is the server's local time with no offset; UTC needs the server's zone
- [ ] Time arithmetic in `--since`/`--until` (`T-2h`)
- [ ] Fold `k8s context list` and the `aks` domain into one `k8s cluster list|connect` (a row per scope with its context, AKS name and namespaces)
- [ ] The k8s tests use `scripts/fake/kubectl`'s built-in cluster, so a k8s flag edits the fake: move those objects into a file under `crates/k8s/` that the fake reads
- [ ] The skill's `paths` hide it until a matching file is touched, so agents found it through `AGENTS.md` and read it with `cat`; drop `paths` if it should be offered from the first turn

## Built only when asked

- [ ] `ado pr list --build succeeded|failed|running|none`: the PR search carries no build status, so it costs one policy-evaluations read per PR (what `pr get` does); cap it
- [ ] `airflow run cancel`: one destructive `PATCH` of the run's state to failed
- [ ] Live tests, off by default: `AGENT_CLI_TEST_AIRFLOW=1` with a `scripts/airflow-up.sh` running Airflow's pinned docker-compose (a smoke DAG that passes, fails and maps; one broken DAG file), and `AGENT_CLI_TEST_DATADOG=1` running every dd read with `--limit 1` against the sandbox in `docs/first-live-run.md`
- [ ] A raw-tools trial (kubectl, az, curl and pup against agent-cli on the cross-domain tasks): a domain earns its place if agent-cli wins at least half its tasks, by 20% of tokens or on correctness

## Chains, round 2 (docs/plans/chains-2.md)

- [ ] 1. `[next: …]` notes on `ado run get|create`, `ado pr get`, `airflow run get`, `dd monitor get`, `k8s pod get`; world traces walked by notes alone
- [ ] 2. Failures to a line: `airflow import-error get` `repo_file`, `ado run get` `errors[{message,at}]`, `ado test list`, `file get` resolving a frame path by suffix
- [ ] 3. `at` on `dd log list`/`span list` from `error.stack`; `ado commit list REPO[:PATH]` with `pr` and `diff`
- [ ] 4. `k8s pod list --kv`, `k8s deployment list --image`
- [ ] 5. `k8s deployment wait`
- [ ] 6. Several ids: `ado file get`, `ado thread update`
- [ ] The trial after phase 2, then after phase 5 (flows F8 to F13)
