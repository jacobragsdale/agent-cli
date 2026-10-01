# airflow: Apache Airflow 3

DAGs, their source, runs, tasks, logs, XComs, import errors, pools,
variables, connections over Airflow 3's REST API (`/api/v2`):
https://airflow.apache.org/docs/apache-airflow/stable/stable-rest-api-ref.html

## Config
`[[airflow.instance]]`: `name` (what `--instance` takes; defaults to the
only one through `pick`), `base_url`, `read_only` (refuses every change),
`k8s_scope` and `k8s_namespace` (where KubernetesExecutor task pods run, so
a row's `pod` is a k8s id), `dags_repo` (`REPO[:FOLDER]` in Azure DevOps:
`source get` prints `repo_file`). Credential: `username` with `password`,
`password_env` or `password_cmd` (signs in at `POST {base_url}/auth/token`),
or `token`, `token_env` or `token_cmd` (a bearer as it is: Astro, Composer,
MWAA). The token goes only to URLs under `base_url/` (`same_origin`), not
`host_under`: a compose Airflow is `http://localhost:8080`.
`connection list` matches `[[sql.connection]]` hosts for `sql_conn`.

## Ids
`dag`, `dag/run`, `dag/run/task[:map][/try]`; `dag/latest` is the newest run
by `run_after`. `--dag` and `--run` stand in for leading pieces; a custom run
id holding `/` still parses. Parsed by `Ref::parse`, resolved by
`Client::resolve`. A DAG file line: `dag:line[-line]` (`task logs`' `at`);
an XCom: `dag/run/task[:map]@key`.

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`: its args, rows, handler, `command!`
and tests (`run/retry.rs` is `airflow run retry`). Copy a sibling.
- `lib.rs`: `DOMAIN`, its `commands` in listing order. `doctor.rs`:
  status and doctor.
- `client.rs`: `Airflow::load(config)`, `open(ctx, instance)` or `locate`
  for a `Client`, which has `get(path)`, `public(path)` (no credential),
  `preview(path, body)` (a POST that only reads), `change(...)` (a write),
  `list(...)` (offset paging, 100 a page), `writable()`, `resolve(&mut Ref)`.
  Row helpers: `text`, `stamp`, `seconds`, `ti_id`, `note_more`,
  `Instance::pod`.
- `dag_run.rs`: `RUN_STATES`, `cleared_ids`.
- `<resource>/mod.rs`: what its verbs share (`run/mod.rs`: the run row,
  `RunIdArgs`; `task/mod.rs`: the task row and id args).
  `instance/mod.rs`: `check_base_url`; `source/mod.rs`: `failing_line`,
  `repo_file`.
- `testing.rs`: `airflow`, `airflow_with`, `paths`, `dry_run`, `API`,
  `CONFIG`, `TOKEN`, and sample `dag`, `run`, `ti` rows.

## Fixtures
`fixtures/world/http/airflow.json` (`/api/v2` and `/auth/token`); facts
`fixtures/world/facts/airflow.md`; the failed-DAG trace and chains flows
in `crates/cli/tests/world_{cross,airflow}.rs`; queries `search.toml`.

## Quirks
- A clear's `dry_run` **defaults to true** on the server: a retry sends
  `"dry_run": false`, a preview `true` through `preview`.
- Airflow caps a page at `maximum_page_limit` (100 unless raised); logs
  page with a token passed as a query value, never a URL to follow.
- `task logs`' `at`: the file the log's `Filling up the DagBag from` line
  names, else one read for `relative_fileloc`.

## Never needed
Other crates' sources, `PLAN.md`, `docs/plans/`, `docs/reference/`.
