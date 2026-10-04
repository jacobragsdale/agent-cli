# TODO

Open work only. What is built is in the code, the crate cards and
`docs/explanation/design.md`.

## Live runs

- [ ] Work through `docs/first-live-run.md`: sql, ado (a sandbox organization, 2026-10-02) and airflow (local 2.9.3 and 3.3.2, `scripts/airflow-up.sh`, 2026-10-03) have run against real services; kv, acr, aks, k8s and dd have not
- [ ] Then the one-line global CLAUDE.md note that agent-cli exists (the README gives the line)

## Phase 5: consolidate

- [ ] After the live runs, the TUIs depend on agent-cli's domain crates and delete their copies and their CLIs; each TUI's tests pass on the shared crates
- Not ported, on purpose: ticket-tui `sync` (no local database), `status` (a TUI status-bar line) and `agent launch|prompt|list` (Herdr orchestration; `ado workitem get` gives an agent the content); az-tui `setup --write` (agents get `aks cluster list`, and the README shows the TOML); sql-bench's replay flags and `--format table|csv` (output is JSON only)

## Trials

- [ ] Development trials: tokens fell 29% on one run per task but 18% on two, short of the 25% target (`docs/trials/dev-after-2026-09-30.md`). Each call carries about 27k tokens of fixed context, so cut calls: a scaffold that fills in more of a routine command, a check that builds once for both feature sets. Next round: three runs per task

## Small follow-ups

- [ ] sql `object list` `modified` is the server's local time with no offset; UTC needs the server's zone
- [ ] Time arithmetic in `--since`/`--until` (`T-2h`): Haiku tried `--until 2d-23h` and `--since 2d-2h` in the chains-2 trial (each an exit 2 it recovered from)
- [ ] Fold `k8s context list` and the `aks` domain into one `k8s cluster list|connect` (a row per scope with its context, AKS name and namespaces)
- [ ] The k8s tests use `scripts/fake/kubectl`'s built-in cluster, so a k8s flag edits the fake: move those objects into a file under `crates/k8s/` that the fake reads
- [ ] The skill's `paths` hide it until a matching file is touched, so agents found it through `AGENTS.md` and read it with `cat`; drop `paths` if it should be offered from the first turn

## ADO query efficiency (after the work rollout)

Projects of thousands of work items and hundreds of people. Measure first, then cut.

- [ ] Count the requests and bytes each command sends (a stderr note under a debug env var through `ctx.env`), then run the pm skill's sweep and the trial tasks against the work org to find the expensive commands
- [ ] `workitem list` asks WIQL for `$top=20000` ids only to print `[50 of N]`: a broad query returns thousands of ids every call. Try `$top=limit+1` with `[50 of 51+]`, or keep the exact count only when it is cheap
- [ ] `workitemsbatch` asks for only the fields the row prints (check `fields` against `--fields`), 200 ids a call
- [ ] Cache identity lookups (`@me`, `--assignee` names) like types and sprints, so a sweep doesn't search identities on every call
- [ ] The fan-outs in `activity list` (updates for 50 work items, commits from 20 repositories) and `person list`: batch them, or give them a `--since` cut-off
- [ ] Rate limits: ADO meters TSTUs (200 per user per sliding 5 minutes); record `X-RateLimit-Remaining` and `X-RateLimit-Delay` in the debug note so a sweep that comes near the limit shows up before it is throttled
- [ ] Incremental reads for the pm sweep: `[System.ChangedDate] > last sweep` plus the cached rows, if sweeps get repeated within a day

## Audit 2026-10-02: open findings

Each has its proposed fix; the report lives outside the repository.

- [ ] IFC-23: an unknown flag (`--sqlfile q.sql`) is reported as "unexpected argument q.sql": name the flag token
- [ ] IFC-31: `--fields tags.x` below a list of strings returns `tags` unchanged: a path below a scalar matches nothing, with the note
- [ ] IFC-36: no version is printed anywhere (a version in doctor's core row)
- [ ] IFC-39: `search` accepts globals it ignores (`--fields`): refuse them
- [ ] IFC-46: a write's `--dry-run` plan ignores `--output`: refuse the pair
- [ ] INP-33: an empty `*_env` name (`token_env = ""`) is accepted and reads "export , or …": refuse it

## Built only when asked

- [ ] `ado pr list --build succeeded|failed|running|none`: the PR search carries no build status, so it costs one policy-evaluations read per PR (what `pr get` does); cap it
- [ ] The ceilings ado's work item commands left (each a `ponytail:` comment): `sprint get` counts Monday to Friday (read teamsettings' working days); `workitem update --above/--below` ranks with parentId 0 (a parent's id for an item nested under it); `person list` reads one page of 1,000 members a team; `attachment get` reads at most core's 32 MiB of an answer, ADO keeps 60 MB (stream to `--output`); `activity list` reads updates for at most 50 work items and commits from at most 20 repositories
- [ ] `airflow run cancel`: one destructive `PATCH` of the run's state to failed
- [ ] An Airflow 2 session-cookie cache (0600, keyed by `base_url` and user), if a work instance accepts only session auth: today each command signs in through the form, and the webserver allows about five sign-ins per 40 s
- [ ] Oracle by SID (`sid` beside `service`, `(CONNECT_DATA=(SID=…))`), if a work database is reached only by SID
- [ ] A live test, off by default: `AGENT_CLI_TEST_DATADOG=1` running every dd read with `--limit 1` against the sandbox in `docs/first-live-run.md`
- [ ] Commands that run a whole chain (an `airflow task triage`: the error, its source lines, the upstream XCom and `repo_file` in one row), for a flow a trial shows still costs Haiku 15 or more calls with the notes in place
- [ ] An output mode of one bare value per line, for `for id in $(…)` without `jq`, if scripts become a goal: a global flag (not `--lines`, a `--tail` synonym) when `--fields` names one scalar
- [ ] A `[datadog]` service-to-repository map, if the live org lacks the source code integration and a dd `at` without git tags (`PATH:LINE`) proves too little
- [ ] A raw-tools trial (kubectl, az, curl and pup against agent-cli on the cross-domain tasks): a domain earns its place if agent-cli wins at least half its tasks, by 20% of tokens or on correctness
