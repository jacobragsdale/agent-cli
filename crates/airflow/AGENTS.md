# airflow: Apache Airflow 3

DAGs, runs, task instances, logs and import errors over Airflow 3's REST API
(`/api/v2`): https://airflow.apache.org/docs/apache-airflow/stable/stable-rest-api-ref.html

## Config
`[[airflow.instance]]`: `name` (what `--instance` takes; defaults to the
only one through `pick`), `base_url`, `read_only` (refuses every change),
`k8s_scope` and `k8s_namespace` (where KubernetesExecutor task pods run, so
a row's `pod` is a k8s id). Credential: `username` with `password`,
`password_env` or `password_cmd` (signs in at `POST {base_url}/auth/token`),
or `token`, `token_env` or `token_cmd` (a bearer as it is: Astro, Composer,
MWAA). The token goes only to URLs under `base_url/` (`same_origin`), not
`host_under`: a compose Airflow is `http://localhost:8080`.

## Ids
`dag`, `dag/run`, `dag/run/task[:map][/try]`; `dag/latest` is the newest run
by `run_after`. `--dag` and `--run` stand in for leading pieces; a custom run
id holding `/` still parses. Parsed by `Ref::parse`, resolved by
`Client::resolve`.

## Where things are
- `lib.rs`: `DOMAIN`, whose `commands` registers every command (its order
  is the listing's), status, doctor, `instance list`, and `testkit`
  (`airflow`, `airflow_with`, `paths`, `dry_run`, `API`, `CONFIG`, `TOKEN`).
- `client.rs`: `Airflow::load(config)`, `open(ctx, instance)` or `locate`
  for a `Client`, which has `get(path)`, `public(path)` (no credential),
  `preview(path, body)` (a POST that only reads), `change(...)` (a write),
  `list(...)` (offset paging, 100 a page), `writable()`, `resolve(&mut Ref)`.
  Row helpers: `text`, `stamp`, `seconds`, `ti_id`, `note_more`,
  `Instance::pod`.
- `dag.rs`: dag list, get, update; import errors.
- `run.rs`: run list, get, create, wait, retry.
- `task.rs`: task list, get, logs, retry.

## Fixtures
`fixtures/world/http/airflow.json` (`/api/v2` and `/auth/token`); facts
`fixtures/world/facts/airflow.md`; the failed-DAG trace in
`crates/cli/tests/world_cross.rs`; queries `crates/airflow/search.toml`.

## Quirks
- A clear's `dry_run` **defaults to true** on the server: a real retry must
  send `"dry_run": false`, and previews send `true` through `preview`.
- A `read_only` instance refuses changes with exit 2 before anything is
  sent (`writable`).
- Airflow caps a page at `maximum_page_limit` (100 unless raised).
- Logs page with a continuation token passed as a query value; Airflow
  never hands back a URL to follow.

## Never needed
Other crates' sources, `PLAN.md`, `docs/plans/`, `docs/reference/`.
