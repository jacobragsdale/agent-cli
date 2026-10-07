# controlm — BMC Control-M (7 commands)

| Command | Effect | Summary |
|---|---|---|
| [`controlm job list`](#controlm-job-list) | read | Find failed, running or waiting Control-M jobs by name or time |
| [`controlm job get`](#controlm-job-get) | read | Show a Control-M job's status, and why it waits |
| [`controlm job logs`](#controlm-job-logs) | read | Read the tail of what a Control-M job run printed, or Control-M's log of it |
| [`controlm job retry`](#controlm-job-retry) | write | Rerun a Control-M job run, as Rerun does in Control-M |
| [`controlm job run`](#controlm-job-run) | write | Run a Control-M job, or a whole folder, now and outside its schedule |
| [`controlm definition list`](#controlm-definition-list) | read | Find which Control-M folder defines a job, and what it runs, where and as whom |
| [`controlm definition get`](#controlm-definition-get) | read | Show a Control-M job's definition: its command, schedule and conditions |

### controlm job list

```text
agent-cli controlm job list — Find failed, running or waiting Control-M jobs by name or time
  --name str                    Job name; * matches any characters, a comma separates several
  --folder str                  Folder name, with * and commas as --name
  --server str                  Control-M/Server (data center) name
  --application str             Application, with * and commas as --name
  --host str                    The agent host it runs on
  --status ok|failed|running|waiting|wait-condition|wait-resource|wait-host|wait-user|wait-workload|unknown  Only runs in this status (waiting: any wait-*)
  --held                        Only held jobs
  --since time                  Only runs that started at or after this
  --until time                  Only runs that started at or before this
  --limit int                   (default 50)
  --instance str                The [[controlm.instance]] name; defaults to the only one
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,name,folder,definition,server,status,held,order_date,start,end,executions,host,application,sub_application,type}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli controlm job list --name 'load_*' --status failed --since 1d --fields id,name,status,start
```

### controlm job get

```text
agent-cli controlm job get — Show a Control-M job's status, and why it waits
 *<id> str        The job run: SERVER:ORDER_ID, the id job list prints
  --instance str  The [[controlm.instance]] name; defaults to the only one
Returns: {id,name,folder,definition,server,status,held,order_date,start,end,executions,host,application,sub_application,type,description,cyclic,estimated_start,estimated_end,waiting[]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli controlm job get ctm-prod:00a1b
```

### controlm job logs

```text
agent-cli controlm job logs — Read the tail of what a Control-M job run printed, or Control-M's log of it
 *<id> str         The job run: SERVER:ORDER_ID, the id job list prints
  --instance str   The [[controlm.instance]] name; defaults to the only one
  --tail int       How many of the last lines (0 for all) (default 200)
  --execution int  Which execution's output (1 is the first); the latest by default
  --events         Control-M's own log of the job (ordered, submitted, ended, rerun) instead of what the job printed
Returns: {id,source,execution,lines,kept,text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli controlm job logs ctm-prod:00a1b --tail 50
```

### controlm job retry

```text
agent-cli controlm job retry — Rerun a Control-M job run, as Rerun does in Control-M
 *<id> str        The job run: SERVER:ORDER_ID, the id job list prints
  --instance str  The [[controlm.instance]] name; defaults to the only one
Returns: {id,answer}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli controlm job retry ctm-prod:00a1b
```

### controlm job run

```text
agent-cli controlm job run — Run a Control-M job, or a whole folder, now and outside its schedule
 *<definition> str  The definition: SERVER/FOLDER/JOB, as definition list prints it; JOB may be * for every job in the folder
  --instance str    The [[controlm.instance]] name; defaults to the only one
Returns: {run_id,jobs[{id,name,folder,definition,server,status,held,order_date,start,end,executions,host,application,sub_application,type}]}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli controlm job run ctm-prod/NightlyLoads/load_orders
```

### controlm definition list

```text
agent-cli controlm definition list — Find which Control-M folder defines a job, and what it runs, where and as whom
  --name str      Job name; * matches any characters
  --folder str    Folder name, with * as --name
  --server str    Control-M/Server (data center) name
  --limit int     (default 50)
  --instance str  The [[controlm.instance]] name; defaults to the only one
Returns: [{id,server,folder,name,type,description,host,run_as,application,sub_application,command}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli controlm definition list --name 'load_*' --fields id,command,host
```

### controlm definition get

```text
agent-cli controlm definition get — Show a Control-M job's definition: its command, schedule and conditions
 *<id> str        The definition: SERVER/FOLDER/JOB, as definition list and job list print it
  --instance str  The [[controlm.instance]] name; defaults to the only one
Returns: {id,server,folder,name,type,description,host,run_as,application,sub_application,command,definition}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli controlm definition get ctm-prod/NightlyLoads/load_orders
```
