# dd — Datadog (20 commands)

| Command | Effect | Summary |
|---|---|---|
| [`dd log list`](#dd-log-list) | read | Search Datadog logs in a time window, newest first, as compact rows |
| [`dd log-count list`](#dd-log-count-list) | read | Count Datadog logs grouped by facets such as service or status |
| [`dd metric list`](#dd-metric-list) | read | Find Datadog metric names reporting recently, by part of the name |
| [`dd metric get`](#dd-metric-get) | read | Query a Datadog metric: min, max, avg, last and a few points per series |
| [`dd monitor list`](#dd-monitor-list) | read | List Datadog monitors alerting or not, by state or tag, with when each triggered |
| [`dd monitor get`](#dd-monitor-get) | read | Show a Datadog monitor: its query, state, the groups that triggered, downtimes |
| [`dd downtime list`](#dd-downtime-list) | read | List Datadog downtimes: which monitors are muted, for what scope and until when |
| [`dd downtime create`](#dd-downtime-create) | destructive | Mute a Datadog monitor for a bounded time (--for, at most 7 days) |
| [`dd downtime cancel`](#dd-downtime-cancel) | write | Unmute a Datadog monitor now, ending its downtime before it runs out |
| [`dd event list`](#dd-event-list) | read | List Datadog events: deploys, monitor alerts, kubernetes events kept past 1h |
| [`dd service list`](#dd-service-list) | read | List the APM services that send traces to Datadog |
| [`dd service get`](#dd-service-get) | read | Show an APM service's health: request count, error rate, p50/p95/p99 latency |
| [`dd span list`](#dd-span-list) | read | Search APM spans: failing or slow requests, or every span of one trace |
| [`dd incident list`](#dd-incident-list) | read | List Datadog incidents, active and stable by default |
| [`dd incident get`](#dd-incident-get) | read | Show a Datadog incident: state, severity, impact, timeline stamps and fields |
| [`dd host list`](#dd-host-list) | read | List hosts reporting to Datadog: up, last reported, apps and cluster |
| [`dd container list`](#dd-container-list) | read | List containers Datadog sees, with state, image and the k8s pod id |
| [`dd slo list`](#dd-slo-list) | read | List Datadog SLOs with their target and timeframe |
| [`dd slo get`](#dd-slo-get) | read | Show an SLO's measured SLI and error budget left over its window |
| [`dd dashboard list`](#dd-dashboard-list) | read | Find Datadog dashboards by words in the title, with their links |

### dd log list

```text
agent-cli dd log list — Search Datadog logs in a time window, newest first, as compact rows
  <query> str                   Datadog log search syntax, ANDed with the flags: '@http.status_code:502 timeout'
  --since time                  From when (default 15m before --until)
  --until time                  Until when (default now)
  --status emergency|alert|critical|error|warn|notice|info|debug|ok[]  Log status (repeatable)
  --service str                 Datadog service tag
  --env str                     env tag (logs, spans: default [datadog] env)
  --cluster str                 kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str               kube_namespace tag
  --pod str                     A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str              A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --index str[]                 Log indexes to search (default: every index)
  --full                        Whole messages and every attribute
  --limit int                   At most 1000 (one page); count more with dd log-count list (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,time,status,service,host,message,error{kind,message},at,trace_id,pod,attributes}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd log list --service api --status error --since 1h --fields time,message,pod
```

### dd log-count list

```text
agent-cli dd log-count list — Count Datadog logs grouped by facets such as service or status
  <query> str                   Datadog log search syntax, ANDed with the flags
  --by str[]                    Facets to group by: service, status, kube_namespace, pod_name, @http.status_code … (repeatable) (default service)
  --since time                  From when (default 1h before --until)
  --until time                  Until when (default now)
  --status emergency|alert|critical|error|warn|notice|info|debug|ok[]  Log status (repeatable)
  --service str                 Datadog service tag
  --env str                     env tag (logs, spans: default [datadog] env)
  --cluster str                 kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str               kube_namespace tag
  --pod str                     A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str              A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --limit int                   Groups to return, largest first (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{group,count}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd log-count list --status error --by service,kube_namespace --fields group,count
```

### dd metric list

```text
agent-cli dd metric list — Find Datadog metric names reporting recently, by part of the name
  <pattern> str     Part of the metric name: kubernetes.memory, latency
  --since time      Reporting since when (default 1h ago)
  --service str     Datadog service tag
  --env str         env tag (logs, spans: default [datadog] env)
  --cluster str     kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str   kube_namespace tag
  --pod str         A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str  A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --limit int       (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [str]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd metric list kubernetes.memory --namespace web
```

### dd metric get

```text
agent-cli dd metric get — Query a Datadog metric: min, max, avg, last and a few points per series
 *<query> str          A metric query: 'avg:kubernetes.cpu.usage.total{*} by {pod_name}'; arithmetic and .rollup() work
  --since time         From when (default 1h before --until)
  --until time         Until when (default now)
  --service str        Datadog service tag
  --env str            env tag (logs, spans: default [datadog] env)
  --cluster str        kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str      kube_namespace tag
  --pod str            A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str     A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --points int         Points per series after downsampling by bucket mean (0: stats only) (default 12)
  --limit int          Series to return, ordered by --sort (default 20)
  --sort max|avg|last  The stat that orders the series, largest first (default max)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{series,group,pod,unit,min,max,max_at,avg,last,points[],gaps}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd metric get 'avg:kubernetes.cpu.usage.total{*} by {pod_name}' --namespace web --since 3h --fields group,max,last
```

### dd monitor list

```text
agent-cli dd monitor list — List Datadog monitors alerting or not, by state or tag, with when each triggered
  <query> str    Monitor search syntax: 'service:api', 'type:metric', 'title:"error rate"'
  --state str[]  Alert, Warn, "No Data", OK (repeatable; any case)
  --tag str[]    Monitor tags: team:web (repeatable, all must hold)
  --limit int    (default 50)
Returns: [{id,name,state,type,priority,triggered,tags[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd monitor list --state Alert,Warn --tag team:web --fields id,name,state,triggered
```

### dd monitor get

```text
agent-cli dd monitor get — Show a Datadog monitor: its query, state, the groups that triggered, downtimes
 *<id> str      The monitor: its id (4711) or its https://app.<site>/monitors/4711 URL
  --all-groups  Every group, not only those that are not OK
Returns: {id,name,state,type,priority,query,message,tags[],triggered,groups[{name,state,triggered,resolved,pod}],downtimes[{scope[],start,end}],url}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd monitor get 4711 --fields state,triggered,groups
```

### dd downtime list

```text
agent-cli dd downtime list — List Datadog downtimes: which monitors are muted, for what scope and until when
  --monitor int  Only downtimes that name this monitor id
  --all          Include ended and canceled downtimes
  --limit int    (default 50)
Returns: [{id,status,monitor,monitor_tags[],scope,start,end,message}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd downtime list --monitor 4711 --fields id,status,end
```

### dd downtime create

```text
agent-cli dd downtime create — Mute a Datadog monitor for a bounded time (--for, at most 7 days)
 *--for duration       How long, at most 7d: 30m, 2h, 1d
  --monitor int        The monitor to mute
  --monitor-tag str[]  Mute every monitor with these tags instead (repeatable)
  --scope str          The groups to mute: '*' (every group), 'pod_name:api-1', 'env:prod' (default *)
  --message str        Why, as Datadog shows it
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: {id,status,monitor,monitor_tags[],scope,start,end,message}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd downtime create --monitor 4711 --for 30m --message 'deploy v1.4.2' --dry-run
```

### dd downtime cancel

```text
agent-cli dd downtime cancel — Unmute a Datadog monitor now, ending its downtime before it runs out
 *<id> str  The downtime id, from dd downtime list
Returns: {id,canceled}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd downtime cancel 00000000-0000-4000-8000-00000000d001 --dry-run
```

### dd event list

```text
agent-cli dd event list — List Datadog events: deploys, monitor alerts, kubernetes events kept past 1h
  <query> str       Datadog event search syntax: 'source:kubernetes', 'tags:deployment'
  --since time      From when (default 1h before --until)
  --until time      Until when (default now)
  --service str     Datadog service tag
  --env str         env tag (logs, spans: default [datadog] env)
  --cluster str     kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str   kube_namespace tag
  --pod str         A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str  A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --limit int       (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,time,title,source,message,service,pod}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd event list 'source:kubernetes' --namespace web --since 2h --fields time,title,pod
```

### dd service list

```text
agent-cli dd service list — List the APM services that send traces to Datadog
  --env str    env tag (default [datadog] env, else every env)
  --limit int  (default 50)
Returns: [str]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd service list --env prod
```

### dd service get

```text
agent-cli dd service get — Show an APM service's health: request count, error rate, p50/p95/p99 latency
 *<service> str  The service, as dd service list prints it
  --env str      env tag (default [datadog] env, else every env)
  --since time   From when (default 1h before --until)
  --until time   Until when (default now)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: {id,env,since,until,spans,errors,error_rate,p50_ms,p95_ms,p99_ms,resources[{resource,spans,errors,p95_ms}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd service get api --env prod --fields error_rate,p95_ms,resources
```

### dd span list

```text
agent-cli dd span list — Search APM spans: failing or slow requests, or every span of one trace
  <query> str              Datadog trace search syntax, ANDed with the flags: 'resource_name:"GET /orders"'
  --since time             From when (default 15m before --until)
  --until time             Until when (default now)
  --status ok|error[]      Span status (repeatable)
  --min-duration duration  Only spans slower than this: 500ms, 2s
  --trace str              Every span of this trace id (from a log's or span's trace_id)
  --service str            Datadog service tag
  --env str                env tag (logs, spans: default [datadog] env)
  --cluster str            kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str          kube_namespace tag
  --pod str                A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str         A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --limit int              At most 1000 (one page) (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: [{trace_id,span_id,time,service,resource,operation,status,duration_ms,error{type,message},at,pod}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd span list --service api --status error --since 30m --fields time,resource,error,trace_id
```

### dd incident list

```text
agent-cli dd incident list — List Datadog incidents, active and stable by default
  <query> str       Incident search syntax, ANDed with the flags: 'customer_impacted:true'
  --state str[]     active, stable, resolved (repeatable; default active,stable)
  --severity str[]  SEV-1, SEV-2 … (repeatable; 1 means SEV-1)
  --limit int       (default 50)
Returns: [{id,title,state,severity,created,resolved}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd incident list --state active --fields id,title,severity
```

### dd incident get

```text
agent-cli dd incident get — Show a Datadog incident: state, severity, impact, timeline stamps and fields
 *<id> str  The incident: its number (982), its UUID, or its https://app.<site>/incidents/982 URL
Returns: {id,uuid,title,state,severity,customer_impacted,created,detected,resolved,fields,url}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd incident get 982 --fields title,state,fields
```

### dd host list

```text
agent-cli dd host list — List hosts reporting to Datadog: up, last reported, apps and cluster
  <query> str    Host name or tag to match: 'aks-nodepool1', 'env:prod'
  --cluster str  kube_cluster_name tag
  --limit int    (default 50)
Returns: [{id,up,last_reported,apps[],cluster,muted}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd host list --cluster prod --fields id,up,last_reported
```

### dd container list

```text
agent-cli dd container list — List containers Datadog sees, with state, image and the k8s pod id
  --service str     Datadog service tag
  --env str         env tag (logs, spans: default [datadog] env)
  --cluster str     kube_cluster_name tag, e.g. prod (a filter; never defaulted)
  --namespace str   kube_namespace tag
  --pod str         A pod as k8s prints it: CLUSTER/NAMESPACE/POD, NAMESPACE/POD or POD
  --deployment str  A deployment: CLUSTER/NAMESPACE/NAME, NAMESPACE/NAME or NAME
  --limit int       (default 50)
Returns: [{name,state,image,pod,host,started}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd container list --namespace web --fields name,state,image,pod
```

### dd slo list

```text
agent-cli dd slo list — List Datadog SLOs with their target and timeframe
  <query> str  Words in the SLO name
  --tag str[]  SLO tags: team:web (repeatable, all must hold)
  --limit int  (default 50)
Returns: [{id,name,type,target,timeframe,tags[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd slo list --tag team:web --fields id,name,target
```

### dd slo get

```text
agent-cli dd slo get — Show an SLO's measured SLI and error budget left over its window
 *<id> str      The SLO: its id, or its https://app.<site>/slo?slo_id=… URL
  --since time  From when (default: the SLO's own timeframe before --until)
  --until time  Until when (default now)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: {id,name,type,target,timeframe,sli,budget_remaining,breaching,since,until,url}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli dd slo get 0c3fe5a1b2c34d5e8f9a0b1c2d3e4f50 --fields sli,target,budget_remaining
```

### dd dashboard list

```text
agent-cli dd dashboard list — Find Datadog dashboards by words in the title, with their links
  <words> str[]  Words the title must contain, any case
  --limit int    (default 50)
Returns: [{id,title,modified,url}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli dd dashboard list kubernetes --fields id,title,url
```
