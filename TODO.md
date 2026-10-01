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
- [ ] Time arithmetic in `--since`/`--until` (`T-2h`): Haiku tried `--until 2d-23h` and `--since 2d-2h` in the chains-2 trial (each an exit 2 it recovered from)
- [ ] Fold `k8s context list` and the `aks` domain into one `k8s cluster list|connect` (a row per scope with its context, AKS name and namespaces)
- [ ] The k8s tests use `scripts/fake/kubectl`'s built-in cluster, so a k8s flag edits the fake: move those objects into a file under `crates/k8s/` that the fake reads
- [ ] The skill's `paths` hide it until a matching file is touched, so agents found it through `AGENTS.md` and read it with `cat`; drop `paths` if it should be offered from the first turn

## Built only when asked

- [ ] `ado pr list --build succeeded|failed|running|none`: the PR search carries no build status, so it costs one policy-evaluations read per PR (what `pr get` does); cap it
- [ ] `airflow run cancel`: one destructive `PATCH` of the run's state to failed
- [ ] Live tests, off by default: `AGENT_CLI_TEST_AIRFLOW=1` with a `scripts/airflow-up.sh` running Airflow's pinned docker-compose (a smoke DAG that passes, fails and maps; one broken DAG file), and `AGENT_CLI_TEST_DATADOG=1` running every dd read with `--limit 1` against the sandbox in `docs/first-live-run.md`
- [ ] Commands that run a whole chain (an `airflow task triage`: the error, its source lines, the upstream XCom and `repo_file` in one row), for a flow a trial shows still costs Haiku 15 or more calls with the notes in place
- [ ] An output mode of one bare value per line, for `for id in $(…)` without `jq`, if scripts become a goal: a global flag (not `--lines`, a `--tail` synonym) when `--fields` names one scalar
- [ ] A `[datadog]` service-to-repository map, if the live org lacks the source code integration and a dd `at` without git tags (`PATH:LINE`) proves too little
- [ ] A raw-tools trial (kubectl, az, curl and pup against agent-cli on the cross-domain tasks): a domain earns its place if agent-cli wins at least half its tasks, by 20% of tokens or on correctness
