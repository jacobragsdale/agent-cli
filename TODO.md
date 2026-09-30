# TODO

## Build (PLAN.md phases)

- [x] 0. Core runtime: registry, discovery, search, output guard, safety chokepoint
- [x] 1. sql domain (SQL Server + Oracle)
- [ ] 2. ado domain (Azure DevOps)
- [ ] 3. kv, acr, aks, k8s domains
- [ ] 4. Harden: search tuning from agent trials, perf gates, full docs
- [ ] 5. Consolidate: TUIs drop their CLIs and share crates where the port is a superset

## New domains (plans in docs/plans/)

- [ ] Airflow 3.x: list DAGs, run history, run and task status, task logs, trigger, wait
- [ ] Datadog: wrap `pup`, or call the API directly, or generate from the OpenAPI spec (decide from the plan)
- [ ] Cross-domain design: shared references, next-step hints, a time-window flag, triage commands

## Follow-ups found along the way

- [ ] Measure `az account get-access-token` latency while signed in (~255 ms when signed out); add a disk token cache if it costs more than ~300 ms
- [ ] ADO answers bad credentials with a 203 sign-in page: map it to exit 3
- [ ] Config env overrides guess types for keys missing from the file ("123" becomes an int)
- [ ] Search hits render a required flag without its value (`--conn <object>` should read `--conn C <object>`)
- [ ] An unknown table error (SQL Server 208, ORA-00942) should hint `sql object list --conn C PATTERN`
