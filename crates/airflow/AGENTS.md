# airflow: Apache Airflow 2.9+ and 3

DAGs, their source, runs, tasks, logs, XComs, import errors, pools,
variables, connections over Airflow 3's REST API (`/api/v2`) or 2.9's
(`/api/v1`): https://airflow.apache.org/docs/apache-airflow/stable/stable-rest-api-ref.html

## Config
`[[airflow.instance]]`: `name` (`--instance`; the only one by default,
`pick`), `base_url`, `api` (`v1` or `v2`; else `probe` asks
`/api/v2/version` with no credential, a 404 being Airflow 2, cached an
hour, so a wrong password never reads as a wrong version), `k8s_scope`,
`k8s_namespace` (a row's `pod` is a k8s id), `dags_repo` (`REPO[:FOLDER]`,
for `repo_file`).
Credential: `username` with `password`, `password_env` or `password_cmd`,
or `token`, `token_env`, `token_cmd`; none: `Auth::Missing`.
A password is a JWT from `/auth/token` on 3; on 2 HTTP Basic, else (401
or 403) the web form (`sign_in_form`). Only under `base_url/`
(`same_origin`). `connection list` matches
`[[sql.connection]]` hosts for `sql_conn`.

## Ids
`dag`, `dag/run`, `dag/run/task[:map][/try]`; `dag/latest` is the newest
run. `--dag` and `--run` stand in for leading pieces; a run id holding `/`
still parses (`Ref::parse`, `Client::resolve`). A DAG file line:
`dag:line[-line]`; an XCom: `dag/run/task[:map]@key`.

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`: args, rows, handler, `command!`,
tests. Copy a sibling.
- `lib.rs`: `DOMAIN`. `doctor.rs`: status and doctor.
- `client.rs`: `Airflow::load`, `open`, `locate` for a `Client`: `get`,
  `text`, `public`, `health`, `preview` (a POST that reads), `change`,
  `list`, `list_like`, `resolve`, `v1`, `newest_first`; row
  helpers `text`, `stamp`, `seconds`, `note_more`, `Instance::pod`.
  `client/auth.rs`: which API, and sign-in. `client/id.rs`: `Ref`, `ti_id`.
- `dag_run.rs`: `RUN_STATES`, `cleared_ids`; `run/mod.rs`: `run_after`;
  `source/mod.rs`: `dag_source`, `failing_line`, `repo_file`, `dag_file`;
  `dag/mod.rs`: `detail`. `diff.rs`: `Instances` (set diffs), `unmasked`.
  Notes carry `Instance::flag` (`--instance`, when several).
- `testing.rs`: `airflow`, `airflow_v1`, `airflow_with`, `paths`,
  `dry_run_with`, `CONFIG` (3), `CONFIG_V1`, `TWO`, rows `dag_v1`, `ti`.

## Fixtures
`fixtures/world/http/airflow.json`, `fixtures/world/facts/airflow.md`,
`crates/cli/tests/world_{cross,airflow}.rs`; queries `search.toml`. Live:
`scripts/airflow-up.sh` (Airflow 2.9.3, a DAG per state) and
`crates/cli/tests/live_airflow.rs`.

## Quirks
- 3 (FastAPI) ignores unknown query params, 2 (connexion) refuses them:
  send only that version's. Each `v1()` branch says what 2.9 lacks.
- A clear's `dry_run` **defaults to true**: a real retry sends `false`.
- Pages cap at `maximum_page_limit` (100); the log token is a query value.
- 2.9 answers a refused credential with 403, logs as a Python repr
  (`text/plain`), XCom values as `str()` (`from_python`); its session
  sign-in allows 5 per 40 s; the cookie stays in memory.

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`.
