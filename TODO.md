# TODO

## Build (PLAN.md phases)

- [x] 0. Core runtime: registry, discovery, search, output guard, safety chokepoint
- [x] 1. sql domain (SQL Server + Oracle)
- [ ] 2. ado domain (Azure DevOps)
- [ ] 3. kv, acr, aks, k8s domains
- [ ] 4. Harden: search tuning from agent trials, perf gates, full docs
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
