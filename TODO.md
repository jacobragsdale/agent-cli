# TODO

## Build (PLAN.md phases)

- [x] 0. Core runtime: registry, discovery, search, output guard, safety chokepoint
- [x] 1. sql domain (SQL Server + Oracle)
- [ ] 2. ado domain (Azure DevOps)
- [x] 3. kv, acr, aks, k8s domains (not yet run against real Azure: docs/first-live-run.md)
- [ ] 4a. Conventions and prerequisites: `--since/--until`, the id is the ref, convention checks, core items below, a `fixtures` replay feature for trials
- [ ] 4b. airflow and dd domains (in parallel)
- [ ] 4c. Harden: agent trials on the fixture world, search tuning, full docs including "adding a command"
- [ ] 5. Consolidate: TUIs drop their CLIs and share crates where the port is a superset

## New domains (plans in docs/plans/)

- [ ] Airflow 3.x (`docs/plans/airflow.md`): 15 hand-written commands over `/api/v2`; open questions at the end of the plan
- [ ] Datadog (`docs/plans/datadog.md`): a `dd` domain of 20 commands on the REST API; pup stays the documented fallback, not wrapped
- [ ] Cross-domain (`docs/plans/cross-domain.md`): build now — `--since/--until` in core, "the id is the ref", convention checks in `check_registry`
- [ ] Core prerequisites from the plans: JSON output with a non-zero exit, per-command default timeout, `password_env`/`password_cmd` in core, extra redaction headers, rate-limit reset headers
- [ ] Overview ≤ 1 KB is only tested with an empty config; test it with every domain configured

## Follow-ups found along the way

- [ ] Measure `az account get-access-token` latency while signed in (~255 ms when signed out); add a disk token cache if it costs more than ~300 ms
- [ ] ADO answers bad credentials with a 203 sign-in page: map it to exit 3
- [ ] Config env overrides guess types for keys missing from the file ("123" becomes an int)
- [ ] Search hits render a required flag without its value (`--conn <object>` should read `--conn C <object>`)
- [ ] An unknown table error (SQL Server 208, ORA-00942) should hint `sql object list --conn C PATTERN`
- [ ] Core `Failure` should carry the HTTP status (azure parses it back out of the message)
- [ ] A token seam in core's test helpers (azure uses fixed tokens in tests)
- [ ] Two search misses (acr, k8s) land second; "acr 1 registries" pluralization in the overview
- [ ] AGENTS.md says each crate exports `DOMAIN`; crates/azure exports KV, ACR and AKS
- [ ] Live checks for docs/first-live-run.md: ACR `_manifests/{tag}`, the exchange's `tenant`, Key Vault `api-version=7.4`, Resource Graph's AKS version and power state
