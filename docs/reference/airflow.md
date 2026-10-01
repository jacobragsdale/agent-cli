# airflow — Apache Airflow (18 commands)

| Command | Effect | Summary |
|---|---|---|
| [`airflow instance list`](#airflow-instance-list) | read | List the configured Airflow instances (from config, no network) |
| [`airflow dag list`](#airflow-dag-list) | read | List DAGs with their schedule, next run and whether they are paused |
| [`airflow dag get`](#airflow-dag-get) | read | Show a DAG's status: paused, next run, params, schedule and its last five runs |
| [`airflow dag update`](#airflow-dag-update) | write | Pause or unpause a DAG |
| [`airflow source get`](#airflow-source-get) | read | Show a DAG's Python file around a line, as Airflow parsed it |
| [`airflow run list`](#airflow-run-list) | read | List DAG runs, newest first, across DAGs or for one |
| [`airflow run get`](#airflow-run-get) | read | Show a DAG run's state, task counts by state, and the tasks that failed |
| [`airflow run create`](#airflow-run-create) | write | Trigger a DAG run, with a conf and optionally a logical date |
| [`airflow run wait`](#airflow-run-wait) | read | Wait for a DAG run: exit 0 if it succeeded, 1 if it failed, 124 if still going |
| [`airflow run retry`](#airflow-run-retry) | write | Retry the failed and upstream_failed tasks of a DAG run |
| [`airflow task list`](#airflow-task-list) | read | List a DAG run's task instances with their state, try, times and host |
| [`airflow task get`](#airflow-task-get) | read | Show a task instance: its tries, its pod, and what blocks it if it is stuck |
| [`airflow task logs`](#airflow-task-logs) | read | Read the tail of a task instance's log, with the exception that failed it |
| [`airflow task retry`](#airflow-task-retry) | destructive | Clear task instances in any state, and their downstream, so they run again |
| [`airflow xcom list`](#airflow-xcom-list) | read | List the XComs a task instance pushed: their keys, not their values |
| [`airflow xcom get`](#airflow-xcom-get) | read | Show an XCom's value: what a task returned or pushed for its downstream tasks |
| [`airflow import-error list`](#airflow-import-error-list) | read | List DAG files that fail to import, newest first: why a DAG is missing |
| [`airflow import-error get`](#airflow-import-error-get) | read | Show an import error's full stack trace |

### airflow instance list

```text
agent-cli airflow instance list — List the configured Airflow instances (from config, no network)
Returns: [{name,base_url,auth,read_only,k8s_scope,k8s_namespace}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow instance list --fields name,base_url,read_only
```

### airflow dag list

```text
agent-cli airflow dag list — List DAGs with their schedule, next run and whether they are paused
  <pattern> str                 Only DAG ids containing this
  --tag str[]                   Only DAGs with this tag (repeatable; any of them)
  --paused true|false           true for paused DAGs only, false for active ones only
  --last-state queued|running|success|failed  Only DAGs whose last run ended in this state
  --instance str                The [[airflow.instance]] name; defaults to the only one
  --limit int                   (default 50)
Returns: [{id,paused,schedule,next_run,tags[],owners[],file,import_errors}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow dag list --last-state failed --fields id,schedule,next_run
```

### airflow dag get

```text
agent-cli airflow dag get — Show a DAG's status: paused, next run, params, schedule and its last five runs
 *<dag> str       The DAG: its id, or its Airflow UI URL
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,paused,schedule,schedule_text,next_run,next_logical_date,catchup,max_active_runs,owners[],tags[],file,bundle,version,last_parsed,import_errors,description,params[{name,default,description}],recent_runs[{id,state,type,run_after,duration}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow dag get etl_nightly --fields paused,next_run,recent_runs
```

### airflow dag update

```text
agent-cli airflow dag update — Pause or unpause a DAG
 *<dag> str            The DAG: its id, or its Airflow UI URL
 *--paused true|false  true pauses the DAG, false unpauses it
  --instance str       The [[airflow.instance]] name; defaults to the only one
Returns: {id,paused,next_run}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow dag update etl_nightly --paused true
```

### airflow source get

```text
agent-cli airflow source get — Show a DAG's Python file around a line, as Airflow parsed it
 *<source> str    The DAG and a line or range: DAG[:LINE[-LINE]] (etl_nightly:42, what task logs prints as at), or the DAG's Airflow UI URL
  --line str      The line or range, when the id leaves it out: LINE or A-B
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,dag,file,version,lines,text,repo_file}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow source get etl_nightly:42 --fields lines,text,repo_file
```

### airflow run list

```text
agent-cli airflow run list — List DAG runs, newest first, across DAGs or for one
  --dag str                     Only this DAG's runs (default: every DAG)
  --state queued|running|success|failed[]  Only runs in this state (repeatable)
  --type scheduled|manual|backfill|asset_triggered[]  Only runs of this type (repeatable)
  --since time                  Only runs due at or after this (their run_after)
  --until time                  Only runs due at or before this
  --instance str                The [[airflow.instance]] name; defaults to the only one
  --limit int                   (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,state,type,run_after,logical_date,start,end,duration,triggered_by}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow run list --dag etl_nightly --state failed --since 1d --fields id,state,end
```

### airflow run get

```text
agent-cli airflow run get — Show a DAG run's state, task counts by state, and the tasks that failed
 *<run> str       The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
  --dag str       The DAG, when RUN is a bare run id
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,state,type,run_after,logical_date,start,end,duration,triggered_by,user,conf,note,dag_version,tasks,failed[]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow run get etl_nightly/latest --fields id,state,tasks,failed
```

### airflow run create

```text
agent-cli airflow run create — Trigger a DAG run, with a conf and optionally a logical date
 *<dag> str           The DAG: its id, or its Airflow UI URL
  --conf str          The run's conf: a JSON object, or - to read it from stdin
  --conf-file path    The run's conf from a JSON file
  --logical-date str  RFC 3339 or now; a DAG that templates {{ ds }} needs one (none by default)
  --run-id str        A run id of your own; Airflow makes a manual__ one otherwise
  --note str          A note on the run
  --instance str      The [[airflow.instance]] name; defaults to the only one
Returns: {id,state,run_after,logical_date,conf}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow run create etl_nightly --conf '{"day":"2026-09-28"}'
```

### airflow run wait

```text
agent-cli airflow run wait — Wait for a DAG run: exit 0 if it succeeded, 1 if it failed, 124 if still going
 *<run> str       The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
  --dag str       The DAG, when RUN is a bare run id
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,state,done,waited_s,duration,failed[]}
Read. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli airflow run wait etl_nightly/latest
```

### airflow run retry

```text
agent-cli airflow run retry — Retry the failed and upstream_failed tasks of a DAG run
 *<run> str       The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
  --dag str       The DAG, when RUN is a bare run id
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,cleared[]}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow run retry etl_nightly/latest
```

### airflow task list

```text
agent-cli airflow task list — List a DAG run's task instances with their state, try, times and host
 *<run> str                     The run: DAG/RUN (DAG/latest for the newest), or its Airflow UI URL
  --dag str                     The DAG, when RUN is a bare run id
  --state none|removed|scheduled|queued|running|success|restarting|failed|up_for_retry|up_for_reschedule|upstream_failed|skipped|…[]  Only task instances in this state (repeatable)
  --instance str                The [[airflow.instance]] name; defaults to the only one
  --limit int                   (default 50)
Returns: [{id,state,try_number,max_tries,start,end,duration,operator,hostname}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow task list etl_nightly/latest --state failed --fields id,end,hostname
```

### airflow task get

```text
agent-cli airflow task get — Show a task instance: its tries, its pod, and what blocks it if it is stuck
 *<task> str      The task instance: DAG/RUN/TASK[:MAP][/TRY], or its Airflow UI URL
  --dag str       The DAG, when the id leaves it out
  --run str       The run id, when the id leaves it out
  --instance str  The [[airflow.instance]] name; defaults to the only one
  --rendered      Add the rendered template fields (large)
Returns: {id,state,try_number,max_tries,start,end,duration,operator,executor,queue,pool,hostname,pod,note,tries[{try_number,state,start,end,hostname}],blocked_by[{name,reason}],rendered_fields}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow task get etl_nightly/latest/load_orders --fields state,blocked_by,tries,pod
```

### airflow task logs

```text
agent-cli airflow task logs — Read the tail of a task instance's log, with the exception that failed it
 *<task> str      The task instance: DAG/RUN/TASK[:MAP][/TRY], or its Airflow UI URL
  --dag str       The DAG, when the id leaves it out
  --run str       The run id, when the id leaves it out
  --instance str  The [[airflow.instance]] name; defaults to the only one
  --tail int      How many of the last lines (0 for all) (default 200)
Returns: {id,state,error,at,lines,kept,complete,sources[],text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow task logs etl_nightly/latest/load_orders --tail 50
```

### airflow task retry

```text
agent-cli airflow task retry — Clear task instances in any state, and their downstream, so they run again
 *<tasks> str[]    Task instances of one run: DAG/RUN/TASK[:MAP] (repeat for more)
  --dag str        The DAG, when the ids leave it out
  --run str        The run id, when the ids leave it out
  --no-downstream  Clear only these tasks, not the tasks downstream of them
  --instance str   The [[airflow.instance]] name; defaults to the only one
Returns: {run,cleared[]}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow task retry etl_nightly/latest/load_orders --yes
```

### airflow xcom list

```text
agent-cli airflow xcom list — List the XComs a task instance pushed: their keys, not their values
 *<task> str      The task instance: DAG/RUN/TASK[:MAP], or its Airflow UI URL
  --dag str       The DAG, when the id leaves it out
  --run str       The run id, when the id leaves it out
  --instance str  The [[airflow.instance]] name; defaults to the only one
  --limit int     Most rows to return (default 50)
Returns: [{id,key,map,time}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow xcom list etl_nightly/latest/extract_orders --fields id,key
```

### airflow xcom get

```text
agent-cli airflow xcom get — Show an XCom's value: what a task returned or pushed for its downstream tasks
 *<xcom> str      The XCom: DAG/RUN/TASK[:MAP]@KEY (from xcom list), or a task instance's id or URL with --key
  --key str       The key, when the id leaves it out (default return_value)
  --dag str       The DAG, when the id leaves it out
  --run str       The run id, when the id leaves it out
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,key,time,value}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow xcom get etl_nightly/latest/extract_orders@return_value --fields value
```

### airflow import-error list

```text
agent-cli airflow import-error list — List DAG files that fail to import, newest first: why a DAG is missing
  --instance str  The [[airflow.instance]] name; defaults to the only one
  --limit int     (default 50)
Returns: [{id,file,bundle,timestamp,error}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow import-error list --fields id,file,error
```

### airflow import-error get

```text
agent-cli airflow import-error get — Show an import error's full stack trace
 *<id> int        The import error's id, from import-error list
  --instance str  The [[airflow.instance]] name; defaults to the only one
Returns: {id,file,bundle,timestamp,error,stack_trace}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli airflow import-error get 12
```
