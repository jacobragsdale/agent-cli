# TODO

## Build (PLAN.md phases)

- [x] 0. Core runtime: registry, discovery, search, output guard, safety chokepoint
- [x] 1. sql domain (SQL Server + Oracle)
- [x] 2. ado domain (Azure DevOps; not yet run against a real org)
- [x] 3. kv, acr, aks, k8s domains (not yet run against real Azure: docs/first-live-run.md)
- [x] 4a. Conventions and prerequisites: `--since/--until`, the id is the ref, convention checks, core items below, a `fixtures` replay feature for trials
- [x] 4b. airflow and dd domains (in parallel)
- [x] 4c. Harden: agent trials on the fixture world (docs/trials/2026-09-30.md: 24 of 24 correct), search tuning, loose fixture matching for trials, clearer usage errors, Diátaxis docs with a generated command reference
- [ ] 5. Consolidate: TUIs drop their CLIs and share crates where the port is a superset (after the live runs below)

## New domains (plans in docs/plans/)

- [x] Airflow 3.x (`docs/plans/airflow.md`): 15 hand-written commands over `/api/v2`; open questions at the end of the plan
- [x] Datadog (`docs/plans/datadog.md`): a `dd` domain of 20 commands on the REST API; pup stays the documented fallback, not wrapped
- [x] Cross-domain (`docs/plans/cross-domain.md`): build now — `--since/--until` in core, "the id is the ref", convention checks in `check_registry`
- [x] Core prerequisites from the plans: JSON output with a non-zero exit, per-command default timeout, `password_env`/`password_cmd` in core (`Credential`), extra redaction headers, rate-limit reset headers, FastAPI and Datadog error bodies, a duration parser (`Span`), `k8s pod list --label`
- [x] Overview ≤ 1 KB is only tested with an empty config; test it with every domain configured
- [x] Build with each domain: `k8s deployment list`, `pod get` `secret_refs` (CSI classes resolved to kv ids), `--cluster` takes the kube context, `acr manifest get` takes one image reference, `ado run get` returns `commit`/`pr`/`workitems`, ado ids take `#`/`AB#`/URLs, kv ids and URIs, `aks cluster list` names the k8s scope, sql `--conn` defaults to the only connection

## For the airflow and dd builders

- [x] Extend `fixtures/world` (README says how): `http/airflow.json`, `http/dd.json`, their config sections, stand-in `*_env` values in `scripts/trial-env.sh`, facts tied to the rest (a DAG whose task pods run in `prod/web`; a monitor on `service:api` that alerted when the worker began crash-looping)
- [x] airflow's `task` resource collides with ado's `task` synonym: add it to `SHARED_WORDS` with labeled queries for both readings
- [ ] dd: `Request::secret_header` was not built; key headers go in `Request::header` from a `Secret` (host checked first), and core masks `DD-API-KEY`/`DD-APPLICATION-KEY` by name. Build the helper if a second domain wants it
- [ ] airflow: the base-URL token scoping (`base_url + "/"`) stays in the domain until a second domain has a configurable base URL

## Follow-ups found along the way

- [ ] Measure `az account get-access-token` latency while signed in (~255 ms when signed out); add a disk token cache if it costs more than ~300 ms
- [x] ADO answers bad credentials with a 203 sign-in page: map it to exit 3
- [ ] Config env overrides guess types for keys missing from the file ("123" becomes an int)
- [x] Search hits render a required flag without its value (`--conn <object>` should read `--conn C <object>`): now `--title TITLE`
- [x] An unknown table error (SQL Server 208, ORA-00942) should hint `sql object list --conn C PATTERN`
- [x] Core `Failure` should carry the HTTP status (azure parses it back out of the message)
- [x] A token seam in core's test helpers (azure uses fixed tokens in tests)
- [x] Two search misses (acr, k8s) land second: "show the image tags in the registry", "read the password from a k8s secret"; also "which image version is deployed in prod" (acr tag list above k8s deployment list)
- [x] "acr 1 registries" pluralization in the overview
- [x] AGENTS.md says each crate exports `DOMAIN`; crates/azure exports KV, ACR and AKS
- [ ] Live checks for docs/first-live-run.md: ACR `_manifests/{tag}`, the exchange's `tenant`, Key Vault `api-version=7.4`, Resource Graph's AKS version and power state
- [x] Core: a per-command default timeout (`ado run wait` wants ~100 s), and data on stdout alongside a non-zero exit (failed waits, `pr create` with missing links)
- [x] Replace the `cfg!(test)` token seams in ado and azure with `Setup::with_token`
- [ ] ADO API shapes to confirm live: comments `order=desc`, WIQL `$top`, log `startLine` base, approvals `state`/`top`, identity search, pipelines run shape, rev-test failure, auto-complete off (empty GUID)
- [ ] ADO shapes this phase added, to confirm live: WIQL `timePrecision=true` with RFC 3339 literals, builds `minTime`/`maxTime` with `queueTimeDescending`, PR search `searchCriteria.minTime`/`maxTime`, `pullrequestquery` `lastMergeCommit` for tag builds, `triggerInfo["pr.number"]`
- [ ] k8s shapes to confirm live: `get deployments,pods -o json` in one call, the Progressing condition's `lastUpdateTime` as "rolled out", `imageID` digests, SecretProviderClass `objects` YAML with quoted names and aliases
- [x] Search says "(no command matches every word; closest:)" for most natural questions ("why did the build fail"); "show image tags" ranks `acr manifest get` over `acr tag list`
- [ ] Cross-domain items not built yet: `ado run list --commit` / `pr list --commit` (the tag trace made them optional), a generic `ids_round_trip_to_get` test helper and a `handoff.rs` table (the world test covers the deploy trace), `next:` notes, time arithmetic (`T-2h`), fold `k8s context list` into `aks`/`k8s cluster`
- [ ] sql `object list` `modified` is the server's local time with no offset; UTC needs the server's zone
- [ ] The fixture world's `AGENT_CLI_NOW` lives in `scripts/trial-env.sh` and `crates/cli/tests/world.rs`; move it into the world directory if a second world appears

## Parity with the old CLIs (the audit of ticket-tui, az-tui and sql-bench)

- [x] Long text: `-` reads stdin and `--NAME-file PATH` a file wherever long text goes in (work item and PR descriptions and acceptance criteria, both comments, SQL, airflow `--conf`); comments keep ticket-tui's 64 KiB cap; `check_registry` holds every stdin argument to its file flag
- [x] `ado pr list --vote`: the `--reviewer`'s own vote, yours without one (ticket-tui's `reviewer:@me vote:none`)
- [ ] `ado pr list --build succeeded|failed|running|none`: the PR search carries no build or policy status, so it costs one policy-evaluations read per PR (what `pr get` does); build it, capped, if agents ask for it
- [x] `ado run list --requested-by` (`requestedFor`) and `--reason` (`reasonFilter`)
- [x] `ado workitem list --priority` (repeatable) and `--date created|changed`, which date `--since/--until` compare (default changed)
- [x] `kv secret list --tag key[=value]`, `--content-type`, `--managed`; `acr repo list --until` (last pushed before)
- [x] `sql query bench --max-rows`, as `query run` has it
- [ ] ADO shapes the parity filters added, to confirm live: builds `requestedFor` by display name (as `az pipelines runs list --requested-for` sends it) and `reasonFilter`; WIQL `[Microsoft.VSTS.Common.Priority] IN (1, 2)` and `[System.CreatedDate]` under `timePrecision`; PR search `reviewerId` rows carrying the reviewer's own vote (a group member who has not voted is absent, which reads as none)

## From the 2026-09-30 trial (docs/trials/2026-09-30.md)

- [x] Two labeled queries still land second: "v1.4.2 build" (`ado pipeline list` above `ado run list`; a version-shaped word could mean a tag) and t04's whole sentence (`k8s pod logs` above `k8s pod list`). Search now reads a version as `tag` and "how many" as a list, and `k8s pod logs`' summary no longer says "last restart"
- [ ] Next round: three runs per cell, tokens and wall time per run, compared task by task with this baseline
- [ ] The one-line global CLAUDE.md note that agent-cli exists (PLAN phase 4; the README gives the line), once the live runs pass
- [ ] Listings say "1 commands" (`agent-cli ado pipeline`): pluralize the count
