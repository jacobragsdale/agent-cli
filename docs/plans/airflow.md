> History: the original plan (2026-09-29). For the current design read docs/explanation/design.md; for current work read TODO.md.

# Airflow domain

Plan for `agent-cli airflow …` against Apache Airflow 3.x (latest 3.3.2,
2026-09-17). It follows PLAN.md, design.md
(`skillbook/skills/api-cli/references/design.md`, "default N") and
`docs/plans/cross-domain.md` (ids as refs, `--since`, scope defaults, `next:`).
API facts were checked against the 3.0.0, 3.1.0, 3.2.0 and 3.3.2 OpenAPI
specs and source. Bracketed tags point to Sources.

## Summary and recommendation

Hand-write 15 commands in a new `crates/airflow` that call the public REST API
(`/api/v2`) through `Ctx`. They cover DAGs (list, status, next run, params,
pause), runs (history, status with a task summary, trigger, a bounded `run
wait`), task instances with states, task logs (the tail plus the extracted
exception), retries of failed work, and import errors. Support Airflow 3.0+
only: Airflow 2 is end-of-life, and `/api/v1` differs in auth, field names and
log format. Authenticate with `/auth/token` (username and password, which
covers the Simple, FAB and Keycloak managers) or a bearer token from an env
var or a command (Astro, Composer 3, and MWAA via a recipe). Don't wrap
`airflowctl`: it takes ~280 ms to start against ~1 ms, and has no
task-instance, log, clear or wait commands. For Kubernetes, print the pod as a
k8s id and hint at it when Airflow can't read a log. No code spans crates.
Generate the other ~110 operations later, through the growth path.

## What the API gives, and its traps

- **One stable API,** `/api/v2` (FastAPI; 128 operations in 3.3.2, 99 in
  3.0.0); `/api/v1` is gone in 3.0 [upgrading]. Lists take `limit` (default
  50, capped by `[api] maximum_page_limit` = 100) and `offset`, and return
  `total_entries`. `order_by` is a string in 3.0 and an array from 3.1, so
  send one key. `~` as `dag_id` lists runs across all DAGs [spec].
- **FastAPI ignores unknown query params,** so a 3.1+ filter sent to 3.0
  silently matches everything (`has_import_errors` 3.1, runs'
  `dag_id_pattern` 3.2, cursor paging 3.3). Every param used here is in 3.0.0.
- **Trigger** (`POST /dags/{d}/dagRuns`): `logical_date` is a required but
  nullable key. Since 3.0 a REST trigger without it has no data interval, and
  `{{ ds }}` raises KeyError [3.0 notes]. A duplicate `run_id` or
  `logical_date` is 409; import errors are 400. **The route never checks
  `is_paused`**, so a paused DAG's run sits in `queued` [dag_run.py].
- **Clear:** `clearTaskInstances` and `dagRuns/{r}/clear` **default to
  `dry_run: true`**, so a real clear must send `false`. `only_failed` means
  `failed` plus `upstream_failed` [spec, dag.py].
- **Logs** (`GET …/taskInstances/{t}/logs/{try}`, `Accept: application/json`)
  return `{content: StructuredLogMessage[] | string[], continuation_token}`.
  Messages are `{event, timestamp, level, logger, error_detail?, …}` and open
  with a `::group::Log message source details` block. There is no
  server-side tail; the token continues only a running task or a paging
  remote handler [log.py, file_task_handler.py].
- **Ids:** `dag_id` and `task_id` match `^[\w.-]+$`; run ids match
  `allowed_run_id_pattern`, default `^[A-Za-z0-9_.~:+-]+$`. None contains `/`
  [helpers.py, config].

## Commands

**Ids** (cross-domain §1):

| Thing | Id | Example |
|---|---|---|
| DAG | the `dag_id` | `etl_nightly` |
| Run | `DAG/RUN` | `etl_nightly/<run_id>` |
| Task instance | `DAG/RUN/TASK[:MAP][/TRY]` | `etl_nightly/<run_id>/load_orders:3/2` (map index 3, try 2) |

The map index is `:3` because `:` is illegal in task ids and shell-safe (zsh
globs `[3]`). Parsing takes the DAG off the front and `TASK[:MAP][/TRY]` off
the back, so a custom run id containing `/` still parses. Also accepted:
`DAG/latest` (newest by `run_after`) and Airflow 3 UI URLs
(`…/dags/<d>/runs/<r>[/tasks/<t>[/mapped/<m>]]`) whose origin matches a
configured `base_url`, which also picks the instance. Every command takes
`--instance NAME`: the only instance by default, else exit 2 listing them.
Timestamps print as RFC 3339 UTC, whole seconds.

| Path | Effect | Args | REST (under `{base_url}/api/v2`) |
|---|---|---|---|
| `airflow instance list` | read | — | none (config only) |
| `airflow dag list [PATTERN]` | read | `--tag str[]` `--paused bool` `--last-state str` `--limit 50` | `GET /dags?dag_id_pattern=&tags=&paused=&last_dag_run_state=` (stale DAGs excluded by default) |
| `airflow dag get <DAG>` | read | — | `GET /dags/{d}/details`; `GET /dags/{d}/dagRuns?order_by=-run_after&limit=5` |
| `airflow dag update <DAG>` | write | `--paused bool` (required) | `GET /dags/{d}/details`; `PATCH /dags/{d}?update_mask=is_paused` `{"is_paused":…}` |
| `airflow run list` | read | `--dag str` `--state str[]` `--type str[]` `--since` `--until` (on `run_after`) `--limit 50` | `GET /dags/{d or ~}/dagRuns?order_by=-run_after&state=&run_type=&run_after_gte=&run_after_lte=` |
| `airflow run get <RUN>` | read | — | `GET /dags/{d}/dagRuns/{r}`; `GET …/{r}/taskInstances?limit=100&offset=` (pages, ≤1,000) |
| `airflow run create <DAG>` | write | `--conf JSON\|-` `--logical-date RFC3339\|now` `--run-id str` `--note str` | `GET /dags/{d}`; `POST /dags/{d}/dagRuns` `{"logical_date":null,"conf":…}` |
| `airflow run wait <RUN>` | read | `--timeout` (default 100 s) | `GET /dags/{d}/dagRuns/{r}` polled; on failure `GET …/taskInstances?state=failed` |
| `airflow run retry <RUN>` | write | — | `POST /dags/{d}/dagRuns/{r}/clear` `{"dry_run":true,"only_failed":true}` (preview, a read), then `"dry_run":false` |
| `airflow task list <RUN>` | read | `--state str[]` `--limit 50` | `GET /dags/{d}/dagRuns/{r}/taskInstances?state=&order_by=start_date` |
| `airflow task get <TI>` | read | `--rendered` | `GET …/taskInstances/{t}[/{m}]`; `GET …/tries`; unfinished: `GET …/dependencies` |
| `airflow task logs <TI>` | read | `--tail 200` (0 = all) | `GET …/taskInstances/{t}[/{m}]` (try, state; skipped if the id has a try); `GET …/{t}/logs/{try}?map_index=&full_content=true` |
| `airflow task retry <TI>...` | destructive | `--no-downstream` | `POST /dags/{d}/clearTaskInstances` `{"dag_run_id","task_ids":[t or [t,m]],"only_failed":false,"include_downstream":true,"reset_dag_runs":true,"dry_run":true}` (preview), then `false` |
| `airflow import-error list` | read | `--limit 50` | `GET /importErrors?order_by=-timestamp` |
| `airflow import-error get <ID>` | read | — | `GET /importErrors/{id}` |

| Path | Returns (agent order) | Example |
|---|---|---|
| instance list | `[{name,base_url,auth,user,read_only,k8s_cluster,k8s_namespace}]` | `airflow instance list --fields name,base_url,read_only` |
| dag list | `[{id,paused,schedule,next_run,tags[],owners[],file,import_errors}]` | `airflow dag list --last-state failed --fields id,schedule,next_run` |
| dag get | `{id,paused,schedule,schedule_text,next_run,next_logical_date,catchup,max_active_runs,owners[],tags[],file,bundle,version,last_parsed,import_errors,description,params[{name,default,description}],recent_runs[{id,state,type,run_after,duration}]}` | `airflow dag get etl_nightly --fields paused,next_run,recent_runs` |
| dag update | `{id,paused,next_run}` | `airflow dag update etl_nightly --paused true` |
| run list | `[{id,state,type,run_after,logical_date,start,end,duration,triggered_by}]` | `airflow run list --state failed --since 1d --fields id,end` |
| run get | `{id,state,type,run_after,logical_date,start,end,duration,triggered_by,user,conf,note,dag_version,tasks{<state>:n},failed[<ti id>]}` | `airflow run get etl_nightly/latest --fields id,state,tasks,failed` |
| run create | `{id,state,run_after,logical_date,conf}` | `airflow run create etl_nightly --conf '{"day":"2026-09-28"}'` |
| run wait | `{id,state,done,waited_s,duration,failed[<ti id>]}` | `airflow run wait etl_nightly/latest` |
| run retry | `{id,cleared[<ti id>]}` | `airflow run retry etl_nightly/latest --dry-run` |
| task list | `[{id,state,try_number,max_tries,start,end,duration,operator,hostname}]` | `airflow task list etl_nightly/latest --state failed --fields id,end,hostname` |
| task get | `{id,state,try_number,max_tries,start,end,duration,operator,executor,queue,pool,hostname,pod,note,tries[{try_number,state,start,end,hostname}],blocked_by[{name,reason}],rendered_fields}` | `airflow task get etl_nightly/latest/load_orders --fields state,blocked_by,tries` |
| task logs | `{id,state,error,lines,shown,complete,sources[],text}` | `airflow task logs etl_nightly/latest/load_orders --tail 50` |
| task retry | `{run,cleared[<ti id>]}` | `airflow task retry etl_nightly/latest/load_orders --dry-run` |
| import-error list | `[{id,file,bundle,timestamp,error}]` | `airflow import-error list --fields file,error` |
| import-error get | `{id,file,bundle,timestamp,error,stack_trace}` | `airflow import-error get 12` |

**Behaviour to pin in tests:**

- **Status needs no command.** `dag get` gives a DAG's status (`paused`,
  `next_run` from `next_dagrun_run_after`, `import_errors`, last 5 runs).
  `run get` gives a run's: counts by state, and `failed` ids that paste into
  `task logs`. `failed` holds root causes; `upstream_failed` only counts.
- **`run create`** reads the DAG first; a paused DAG gets a note and the hint
  `dag update <d> --paused false`. Success hints `run wait <id>`. Help says to
  pass `--logical-date now` to DAGs that template `{{ ds }}`. 409 is exit 5; a
  400 for import errors hints `import-error list`.
- **Retries preview first** (a read with the server's `dry_run: true`). An
  empty preview returns `cleared: []` and writes nothing. Under `--dry-run` the
  preview list goes to stderr beside core's plan. `task retry` clears the
  named tasks in any state plus their downstream by default, because clearing
  only a failed task leaves its `upstream_failed` children terminal. Its ids
  must share one run (one call), else exit 2.
- **`dag update --paused false`** notes `catchup` and `next_run`: with catchup
  on, unpausing schedules every missed interval.
- **`task get`:** `blocked_by` (from `/dependencies`) answers "why is my task
  stuck in queued"; `rendered_fields` is large, so it needs `--rendered`; a
  mapped task without `:N` is exit 2 with a `task list` hint.
- **Errors:** 401 after one re-mint → exit 3, hint `doctor airflow`. 403 →
  exit 1, hint "the Airflow role lacks this permission". 404 → exit 4, hint
  the matching `list`. 3xx → exit 3, hint "`base_url` is wrong (scheme or
  path prefix)".

**Not building:**

| Idea | Why not |
|---|---|
| Wrap `airflowctl` | Measured: `airflowctl version` median start 279 ms (Python ≥3.10, 38 packages) against 1 ms for agent-cli's overview. 0.1.5 and 1.0.0rc2 have no task-instance, log, clear or wait commands, and keep the token in the OS keyring |
| Airflow 2 (`/api/v1`) | OSS end of life was 2026-04-22. v1 has session/basic auth, `execution_date`, single-key `order_by` and string logs: a second client and fixture set for a closed line. `doctor` detects v1 and exits 3 |
| Experimental `GET …/dagRuns/{r}/wait` (3.1+) | Experimental. It holds an NDJSON stream open, so a proxy idle timeout or our deadline loses the final state, and 3.0 lacks it. Polling is about 20 lines |
| NDJSON log streaming, `--follow` | PLAN forbids follow. JSON already returns the whole log |
| `run cancel` (`PATCH` state=failed) | Not asked for; one destructive PATCH when wanted |
| Mark task, variables, connections, pools, XCom, backfills, assets, DAG versions and sources, event logs | The generated long tail. Connections are Reveal |
| Last-run state in `dag list` rows | Not in `DAGResponse` (only the private `/ui` API). `--last-state failed` answers "which DAGs are failing" |
| A health command | `doctor airflow` covers the metadatabase, scheduler, triggerer and dag-processor |
| `--grep` or `--level` on logs | Wait for trials. `error` plus the tail cover the usual question |

## Search

Domain synonyms apply globally (`search::expand`), so they use only Airflow's
own phrases (cross-domain §5): `dag run`/`dagrun` → run, `task instance` →
task, `workflow`/`data pipeline` → dag, `trigger` → create, `clear`/`rerun` →
retry, `pause`/`unpause` → update, `broken dag` → import-error. `pipeline`,
`job` and `build` stay with ado. ado's `task → workitem` still collides with
the `task` resource, so labeled queries cover both readings.

| Command | Keywords | Labeled queries (`search.toml`) |
|---|---|---|
| instance list | environment, deployment, server, composer, mwaa, astro | "which airflow environments are configured"; "list airflow instances" |
| dag list | dags, paused, tags, schedule, owner, failing | "list all airflow dags"; "which dags are paused"; "which dags failed on their last run" |
| dag get | status, next, params, schedule, recent | "when does the etl_nightly dag run next"; "status of the orders dag"; "what params does this dag take" |
| dag update | pause, unpause, enable, disable, resume | "pause the etl_nightly dag"; "unpause a dag in airflow" |
| run list | history, recent, dagruns, failed, executions | "airflow dag runs that failed in the last day"; "run history of the etl_nightly dag" |
| run get | status, state, progress, summary | "state of the latest etl_nightly dag run"; "which tasks failed in this airflow dag run" |
| run create | trigger, start, kick, conf, manual | "trigger the etl_nightly dag with conf"; "kick off an airflow dag run" |
| run wait | poll, finish, until, done, block | "wait for the airflow dag run to finish"; "block until my dag run completes" |
| run retry | clear, rerun, failed, resume | "rerun the failed tasks of a dag run"; "clear failed tasks in an airflow run" |
| task list | instances, states, steps, mapped, running | "list task instances of a dag run with their states"; "which airflow tasks are still running" |
| task get | stuck, queued, dependencies, tries, attempts, pod, hostname | "why is my airflow task stuck in queued"; "how many attempts did the load task take" |
| task logs | log, output, error, traceback, exception | "show the log of the failed airflow task"; "traceback of the transform task" |
| task retry | clear, rerun, downstream, reset | "clear the load task and its downstream tasks"; "re-run one airflow task" |
| import-error list | broken, parse, syntax, missing, import | "why is my dag missing from airflow"; "dag files with import errors" |
| import-error get | trace, traceback, full | "full stack trace of an airflow import error"; "show import error 12" |

## Config and auth

```toml
[[airflow.instance]]
name = "prod"                                  # what --instance takes
base_url = "https://airflow.contoso.example"   # API server incl. any path prefix, without /api/v2
username = "agent"                             # password auth: POST {base_url}/auth/token
password_env = "AIRFLOW_PROD_PASSWORD"         # or password_cmd = "pass show airflow/prod", or password = "…" (throwaway)
read_only = true                               # refuse every write on this instance
k8s_cluster = "prod"                           # optional: any name `k8s --cluster` accepts
k8s_namespace = "airflow"                      # optional: where task pods run

[[airflow.instance]]
name = "hosted"
base_url = "https://deployment.contoso.example/d-1a2b3c"
token_env = "AIRFLOW_HOSTED_TOKEN"             # bearer auth; or token_cmd = "gcloud auth print-access-token"
```

Only `[airflow]` is read, lazily, with `deny_unknown_fields`. Instances are an
array like `[[sql.connection]]`, which core's `AGENT_CLI_*` overrides (scalars
only) can't reach; an env-only setup points `AGENT_CLI_CONFIG` at a file, and
no secret sits in the file anyway (`*_env`/`*_cmd` only name it). Each
instance has exactly one of `username` plus a password source, `token_env`,
or `token_cmd`. `base_url` must be https, with plain http allowed only to
`localhost`, `127.0.0.1` and `[::1]` (the compose test); a trailing `/api/v2`
is exit 3 with the fix. The overview says `airflow 2 instances`. `doctor
airflow` checks per instance: `/api/v2/monitor/health` without auth
(metadatabase, scheduler, triggerer, dag-processor), `/api/v2/version` (3.x;
a v1 server is exit 3), a token mint plus `GET /dags?limit=1`, and the
import-error count.

**Auth kinds, in build order:**

1. **`password`:** `POST {base_url}/auth/token` `{"username","password"}` →
   201 `{"access_token"}`, sent as `Bearer`. The same body works for the
   Simple manager, FAB (the chart's and compose's default) and Keycloak
   (default `grant_type` is password). Tokens last 24 h (`[api_auth]
   jwt_expiration_time`); mint once per invocation, re-mint on 401 via core's
   `Mint(fresh)`. The mint is a `Request::query`, so it works under
   `AGENT_CLI_READ_ONLY` and `--dry-run`.
2. **`token`:** `token_env` is static; `token_cmd` uses `password_cmd`'s
   resolver and re-runs after a 401.
3. **Later, if asked:** Keycloak `client_credentials`, a native MWAA kind.
   FAB basic auth isn't needed, since `/auth/token` works with FAB.

| Hosting | Airflow 3 | Config | Out of the box |
|---|---|---|---|
| Self-hosted (Helm chart, compose) | Chart 2.0.0 ships 3.3.2 with FAB | `password` | Yes |
| Astronomer Astro | Airflow 3 deployments | `token_env` = a Deployment, Workspace or Org API token; `base_url` = the Deployment URL | Yes |
| Cloud Composer 3 | GA from `composer-3-airflow-3.1.7-build.4` (2026-04-15); 3.2.2 and 3.3.1 builds exist, but the default is still 2.11 | `token_cmd = "gcloud auth print-access-token"`; `base_url` = the web server URL | Yes (starts gcloud on every call) |
| Amazon MWAA | 3.2 (2026-05), 3.3.1 (2026-09) | `token_cmd` = the recipe below (its `_token` cookie is a 12 h JWT) | With the recipe (untested) |

```sh
aws mwaa create-web-login-token --name ENV --query '[WebServerHostname,WebToken]' --output text |
  { read -r host tok; curl -sf -c - -o /dev/null --data-urlencode "token=$tok" "https://$host/pluginsv2/aws_mwaa/login" | awk '$6=="_token"{print $7}'; }
```

MWAA's `InvokeRestApi` isn't used: it needs SigV4 and the AWS credential
chain, and caps each call at 10 s and 6 MB.

**Secrets.** Passwords and tokens live only in `Secret`; core's `redact`
already masks Bearer values and JWTs in errors and plans, and `instance list`
shows only the auth kind and user. `host_under` doesn't fit (https only, no
ports, so `localhost:8080` fails), but every Airflow URL is built from
`base_url`: paging is by offset, the log token is a query value, and the
server never hands back a URL. So the one request builder attaches the token
only to URLs starting with `base_url + "/"` (move it to core when a second
domain has a configurable base URL). Airflow masks secrets in logs and
rendered fields but not in `conf`, so `run get` passes `conf` through
`redact_value`. Cost: password auth adds a POST per call, `token_cmd` a
process start (gcloud and aws are Python, 0.5–1 s). Measure both live; above
~300 ms, reuse PLAN's planned expiry-aware 0600 token cache for `az` (a JWT's
`exp` is readable).

## Time budgets and log size

**Deadlines:** 60 s by default. `run wait` defaults to 100 s, like `ado run
wait` (a core prerequisite). Retries and triggers return at once, since the
scheduler does the work; their hint is `run wait`.

**`run wait`** polls at 2 s, growing ×1.5 to 10 s, and never sleeps past the
deadline. It keeps no state, so re-running it resumes the wait.

| Exit | When | Output and hint |
|---|---|---|
| 0 | Success | |
| 1 | Failed | JSON with `failed` ids; hint `airflow task logs <first failed id> --tail 200` |
| 124 | Still running or queued | JSON with the current state; hint "the run keeps going; run the same command again". A still-`queued` run whose DAG is paused (one extra GET) gets an unpause hint |

**`task logs`** sends `Accept: application/json` and `full_content=true`, and
follows `continuation_token` only for a finished task within the deadline
(Elasticsearch and OpenSearch page); a running task gets one answer,
`complete: false` and a note. The source-details group becomes `sources`
("Could not read served logs: …"). Each message becomes one line,
`2026-09-28T00:43:55.120Z ERROR task: <event> key=value…` (ms kept, per
cross-domain), with `error_detail` as `ExcType: value` plus `file:line in fn`
frames. `error` is the last exception's type and value with its innermost
frame outside `site-packages` (the UI's user-code heuristic), or the last
traceback's final line for plain-string logs. It keeps the last `--tail` lines
and reports `lines` and `shown`; core's 12 KB guard then cuts from the front
for `logs`, saves the full text to a temp file, and `--raw` bypasses it. The
transport's 32 MB body cap bounds the fetch (beyond it, exit 1 hinting
`externalLogUrl`). Try 0 (skipped, `upstream_failed`) returns Airflow's
one-line message.

## Safety

| Command | Effect | Why | `--dry-run` shows |
|---|---|---|---|
| `run create` | write | Additive, the requested action, like `ado run create`. It runs the DAG's side effects, so guard prod with `read_only = true` or `AGENT_CLI_READ_ONLY` | The DAG read and paused note, then the POST body |
| `dag update` | write | One flag, trivially reversed | The PATCH, plus the catchup note on unpause |
| `run retry` | write | Re-runs only `failed` and `upstream_failed` work, like `ado run retry` | The server's preview list as a note, then the POST |
| `task retry` | destructive (`--yes`) | Resets tasks in any state (successes and their downstream too). That re-executes completed, possibly non-idempotent work and re-queues the run | Same as `run retry` |
| all others | read | | |

The token mint and both clear previews go through `ctx.read` as
`Request::query`. Per-instance `read_only` is enforced in the domain, as sql
does per connection. A fixture asserts the real clear sends `"dry_run":
false`.

## Kubernetes synergy

**Facts** (cncf.kubernetes provider, Helm chart):

- **KubernetesExecutor** pods carry labels `dag_id`, `task_id`, `run_id`,
  `try_number`, `map_index`, `kubernetes_executor=True`, `airflow-worker` and
  `airflow_version`. Values pass through `make_safe_label_value` (an unsafe
  value becomes 53 chars plus an md5 suffix); annotations keep the raw ones.
  The namespace is `[kubernetes_executor] namespace` (Helm: the release's)
  unless `pod_override` sets one. The task instance's `hostname` is the pod
  name; the executor itself logs "Attempting to fetch logs from pod
  {ti.hostname}".
- **Retention:** with `delete_worker_pods=True`,
  `delete_worker_pods_on_failure=False` keeps a pod only "where the worker
  itself failed, not when the task it ran failed". An ordinary failure leaves
  no pod; OOMKilled, eviction and image-pull failures do. Airflow reads the
  pod log (container `base`) only while the task runs; afterwards the log
  survives only in remote logging or a shared volume (the chart's
  `logs.persistence` is off by default). Events expire after 1 h
  (cross-domain §3).
- **CeleryExecutor** (the chart's default): workers are a StatefulSet, and
  `hostname` may be the FQDN `<release>-worker-0.<svc>.<ns>.svc.cluster.local`.
  Task logs are worker files served on port 8793. The pod log is the
  worker's, but its restarts and events explain a task that died without a
  log.
- **KubernetesPodOperator** pods are labelled `dag_id`, `task_id`, `run_id`,
  `try_number` and `kubernetes_pod_operator=True`, and deleted on finish by
  default.

**Design** (no Cargo edge; airflow never reads `[k8s]`):

1. Task rows print `hostname` (cross-domain Q2).
2. `task get` adds `pod` = `<k8s_cluster>/<k8s_namespace>/<hostname before the
   first dot>` when both keys are set and `hostname` exists. That is the id
   form `k8s pod logs` and `k8s event list --pod` take.
3. When a failed task's `sources` show Airflow couldn't read the log, `task
   logs` hints `agent-cli k8s pod logs <pod> --tail 200` and `agent-cli k8s
   event list --pod <pod>`. They only help while the failure is live; for
   last night, the chain is airflow → dd (cross-domain §3).
4. Pods that never started (queued, no `hostname`; Pending or
   ImagePullBackOff) need the k8s plan to add `k8s pod list --label k=v`
   (kubectl `-l`). The hint `--label dag_id=<d> --label task_id=<t> --label
   try_number=<n>` prints only when both ids are already label-safe, so no md5
   dependency.
5. An automatic fallback is `airflow run triage` (cross-domain §4) via the
   read-only `Ctx::call("k8s pod logs", …)`. Build it only if trials show the
   chain repeating.
6. Once k8s exists, cross-domain test 6 checks that every printed `agent-cli
   k8s …` hint parses.

## Hand-written vs generated

Hand-write these 15, because generated 1:1 operations can't express them.
They are composites (`run get`, `dag get`, `logs`, preview-then-write retries,
the `wait` loop). They cut fields: `DAGResponse` has 30 and
`TaskInstanceResponse` 33, while our rows have 8–11. They encode traps the
spec doesn't: the `dry_run` default, the paused trigger, the null
`logical_date`, `~` and 3.0-safe params. And they use our ids and the closed
`VERBS`, which operationIds like `post_clear_task_instances` don't fit.

Generate the other ~110 operations when PLAN's generic runner lands:
variables, pools, connections (Reveal), XCom, assets, backfills, event logs,
DAG versions, sources, warnings and stats, providers, plugins, jobs, config.
The generator skips the hand-written operationIds, maps tags to resources, and
renames time and limit params to the canonical flags (cross-domain §6).

## Core prerequisites

Small, and shared with other domains:

1. A command can print JSON **and** exit non-zero: `run wait` needs 1 and 124,
   as does `ado run wait`. Today a `Failure` prints no stdout.
2. A per-command default timeout (`run wait` 100 s), as PLAN gives `ado run
   wait`.
3. `failure_message` reads FastAPI's `{"detail":"…"}` and 422's
   `{"detail":[{"loc":[…],"msg":"…"}]}` instead of printing the raw body.
4. sql's `password`/`password_env`/`password_cmd` resolver moves to core, and
   airflow reuses it for `token_env` and `token_cmd`.
5. From the cross-domain plan: the `--since`/`--until` type and the
   scope-default helper. For the k8s plan: `k8s pod list --label`.

## Testing

**Fixture tests** (`testing::run`, `FakeTransport`) use synthetic JSON shaped
by the 3.3.2 schemas at `https://airflow.contoso.example`. They cover every
command's happy path, paging and the `[50 of 312; --limit N]` note; ids (`/`
inside a run id, `:3/2`, `latest`, a UI URL picking the instance, an unknown
host → exit 2); 401 then re-mint, with the token never in stdout, stderr or a
plan; the paused-trigger note, 409 → 5, 400 for import errors; `"dry_run":
false` on the real clear and no write after an empty preview; `run wait`
success, failed (exit 1 with JSON) and 124 (the poll interval is a parameter,
zero in tests); log rendering over structured, `error_detail` and
plain-string content, the `::group::` split, `--tail` and the guard on a
generated 5,000-line log; `blocked_by`, and `pod` with and without the k8s
keys; dry-run tests for the 4 non-read commands, read-only refusal, and 30
search queries.

**Live test** (`AGENT_CLI_TEST_AIRFLOW=1`, `scripts/airflow-up.sh`) runs the
official 3.3.2 `docker-compose.yaml`, fetched pinned rather than vendored. It
is heavy: postgres, redis, the API server, scheduler, dag-processor, a Celery
worker and the triggerer plus an init job, on a 661 MB compressed image, with
≥4 GB of Docker RAM (its own check) and minutes to healthy on first pull. It
exercises the realistic paths, FAB auth (`airflow`/`airflow`, throwaway,
localhost-only) and the Celery log server. Setup: `LOAD_EXAMPLES=false`, a
mounted `agent_cli_smoke` DAG (a pass, a fail with a real traceback, a mapped
task of 3, a param) and one broken DAG file. Flow: dag list → run create →
run wait (exit 1) → task list → task logs (`error` set) → task retry → pause
and unpause → import-error list. Run it locally, on demand. The lighter
option is one `apache/airflow:3.3.2 standalone` container, which covers the
Simple manager's `/auth/token`.

**Agent trial** (PLAN), 6 tasks: which DAGs failed in the last day; why X
failed (quote the error); trigger X with a conf and report success; retry the
latest run's failed tasks; why DAG Y is missing; when Z runs next and whether
it's paused.

## Effort

`crates/airflow` is about 1,500 lines: config, auth, ids and client ~450,
dag ~200, run ~400, task with log rendering ~350, import-error, instance and
doctor ~150. Tests add ~900, and the core prerequisites ~120 unless ado lands
them first. That's about 4 days for one agent, plus half a day of core work:
day 1 config, auth, ids, doctor, dag, import-error; day 2 run list, get,
create, wait; day 3 task list, get, logs, retries, dry-run tests, search
queries; day 4 the live compose run, fixes and the agent trial.

## Decided (2026-09-29)

- **Airflow 3.x only.** The user's 2.x environments stay out of scope.
- **Hosting:** self-hosted on AKS with the official Helm chart, as three
  instances: dev, qa and prod. Each `[[airflow.instance]]` gets an optional
  `k8s_scope` naming the `[[k8s.scope]]` its task pods run in, so `task get`
  prints a pod ref that `k8s` accepts. `config.example.toml` shows prod with
  `read_only = true`.
- **Executor:** KubernetesExecutor. It's unknown whether remote logging is on,
  so `task logs` handles both cases:
  - Airflow serves the log (remote logging, or a pod that is still running).
  - Or it says so plainly and prints the k8s pod ref while the pod exists.

  Verify which case applies on the first live run.
- **Unconfirmed defaults, as proposed below:**
  - Safety levels.
  - `run cancel` later.
  - `DAG/latest` kept until the trials show whether agents use it.
  - No Keycloak `client_credentials`.
  - The compose test runs locally only.
- **Trials:** local fixtures plus the local compose Airflow; no real
  environment is reachable.

## Open questions

1. **Airflow 2:** do you need any 2.x environment? Composer 3 still defaults
   to 2.11, and Astro supports 2.x until April 2027. The plan is 3.0+ only.
2. **Hosting first:** Helm/FAB, Astro, Composer or MWAA? The MWAA recipe is
   untested, and a native `mwaa` kind would come only on request.
3. **Executor and remote logging:** which executor runs the tasks
   (Kubernetes, Celery, KPO), and is remote logging on? That decides what the
   pod hints are worth (cross-domain Q4).
4. **Safety:** is `run create` fine as a plain write, `task retry` as
   destructive and `run retry` as a write? Should `config.example.toml` show
   prod with `read_only = true`?
5. **`run cancel`:** now, or later? And do we keep `DAG/latest` if trials
   ignore it?
6. **Keycloak:** is `client_credentials` needed?
7. **CI:** should the compose test run in CI (a public runner fits it,
   taking minutes) or stay local?

## Sources

- [spec] OpenAPI 3.3.2 (3.1.0 and 3.2.0 at the same path; 3.0.0 as `v1-rest-api-generated.yaml`): https://github.com/apache/airflow/blob/3.3.2/airflow-core/src/airflow/api_fastapi/core_api/openapi/v2-rest-api-generated.yaml ; reference: https://airflow.apache.org/docs/apache-airflow/stable/stable-rest-api-ref.html
- [dag_run.py] https://github.com/apache/airflow/blob/3.3.2/airflow-core/src/airflow/api_fastapi/core_api/routes/public/dag_run.py ; [dag.py] `only_failed`: https://github.com/apache/airflow/blob/3.3.2/airflow-core/src/airflow/serialization/definitions/dag.py
- [log.py] https://github.com/apache/airflow/blob/3.3.2/airflow-core/src/airflow/api_fastapi/core_api/routes/public/log.py ; [file_task_handler.py] https://github.com/apache/airflow/blob/3.3.2/airflow-core/src/airflow/utils/log/file_task_handler.py ; UI: https://github.com/apache/airflow/blob/3.3.2/airflow-core/src/airflow/ui/src/components/renderStructuredLog.tsx , https://github.com/apache/airflow/blob/3.3.2/airflow-core/src/airflow/ui/src/router.tsx
- [helpers.py] https://github.com/apache/airflow/blob/3.3.2/airflow-core/src/airflow/utils/helpers.py ; [config] https://airflow.apache.org/docs/apache-airflow/stable/configurations-ref.html
- [3.0 notes] https://airflow.apache.org/docs/apache-airflow/3.0.0/release_notes.html ; [upgrading] https://airflow.apache.org/docs/apache-airflow/stable/installation/upgrading_to_airflow3.html ; Airflow 2 EOL: https://airflow.apache.org/docs/apache-airflow/stable/installation/supported-versions.html , https://www.astronomer.io/airflow-2-eol/
- Auth: https://airflow.apache.org/docs/apache-airflow/stable/security/jwt_token_authentication.html ; https://github.com/apache/airflow/blob/3.3.2/airflow-core/src/airflow/api_fastapi/auth/managers/simple/routes/login.py ; https://github.com/apache/airflow/blob/main/providers/fab/src/airflow/providers/fab/auth_manager/api_fastapi/routes/login.py ; https://airflow.apache.org/docs/apache-airflow-providers-fab/stable/auth-manager/api-authentication.html ; https://github.com/apache/airflow/blob/main/providers/keycloak/src/airflow/providers/keycloak/auth_manager/datamodels/token.py
- Hosted: https://www.astronomer.io/docs/astro/airflow-api ; https://docs.cloud.google.com/composer/docs/composer-3/access-airflow-api ; https://docs.cloud.google.com/composer/docs/release-notes ; https://docs.aws.amazon.com/mwaa/latest/userguide/access-mwaa-apache-airflow-rest-api.html ; https://aws.amazon.com/about-aws/whats-new/2026/09/amazon-mwaa-apache-airflow-3-3-1/
- airflowctl: https://pypi.org/project/apache-airflow-ctl/ ; https://airflow.apache.org/docs/apache-airflow-ctl/stable/index.html (start-up measured locally via `uvx`, 5 runs each)
- Kubernetes: https://github.com/apache/airflow/blob/main/providers/cncf/kubernetes/src/airflow/providers/cncf/kubernetes/pod_generator.py , …/executors/kubernetes_executor.py , …/get_provider_info.py , …/operators/pod.py ; chart: https://github.com/apache/airflow/blob/main/chart/values.yaml
- Compose: https://airflow.apache.org/docs/apache-airflow/3.3.2/docker-compose.yaml ; https://airflow.apache.org/docs/apache-airflow/stable/howto/docker-compose/index.html
