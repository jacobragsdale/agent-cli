# Datadog domain (`dd`)

Researched 2026-09-29. Evidence: pup 1.23.6 (source at commit `9e26544`, and
its release binary run locally with no credentials and no network); Datadog's
OpenAPI v1 and v2 specs from `datadog-api-client-rust`; Datadog's docs (see
Sources); and the trial evidence in
`skillbook/skills/api-cli/references/design.md` ("design.md"). This plan does
not re-argue what `docs/plans/cross-domain.md` already binds: the `When` time
type, "the id is the ref", the canonical flags and `next:` notes.

## Summary and recommendation

**Build a hand-written `dd` domain of 20 commands that call Datadog's REST
API through core's transport. Point agents at pup for everything else. Don't
wrap pup, and don't generate commands from the spec yet.**

**What pup is.** Datadog's own CLI for agents: `DataDog/pup`, Apache-2.0,
Rust. Created 2026-02-03, 1.0.0 on 2026-06-09, then 164 releases up to v1.23.6
(2026-09-29), about five a week. It installs from a Homebrew tap
(`datadog-labs/pack`) or a 29.6 MB tarball holding a 94 MB binary. Its
coverage is far wider than any hand-written set: `pup agent schema` lists 85
top-level groups and 828 leaf commands, and the source calls 92 internal
`/api/ui/` or `/api/unstable/` paths that the public spec lacks (APM service
stats, the flow map). Its auth is good: OAuth2 PKCE with tokens in the OS
keychain, a bearer token in `DD_ACCESS_TOKEN`, or a key pair in `DD_API_KEY` +
`DD_APP_KEY`, plus named org sessions and a trust prompt before it talks to a
host that isn't Datadog's.

**Where pup falls short.** By design.md's measured criteria it is
agent-*aware*, not agent-first:

1. **Discovery is large.** Agent mode switches on by itself when
   `CLAUDECODE=1`. Then `pup --help` prints 1,010,801 bytes of JSON,
   `pup agent schema --compact` 184,865 B, and `pup logs search --help`
   31,928 B (the whole logs subtree). design.md's limits are 1 KB for the
   overview and 2 KB for help, and it rejected a 113 KB help screen. pup has
   no search command.
2. **Agent mode auto-approves destructive commands.** `src/main.rs` sets
   `cfg.auto_approve = true` whenever agent mode is on, so inside Claude Code
   `pup monitors delete 123` runs without `--yes`. design.md rejected any
   mode that switches on when an agent is detected.
3. **`--read-only` only checks the command's last word** against a list of
   write verbs. `pup --read-only monitors delete 123 --yes` is refused. But
   `pup --read-only security findings mute --file x.json` and
   `pup --read-only api -X DELETE v1/monitor/123` both got past the guard and
   stopped only at the missing credential. With one set, they would have
   sent the change.
4. **No `--dry-run`** anywhere in `src/`.
5. **Exit codes carry no meaning.** A missing login, a read-only refusal and
   an API error all exit 1. On a 429 pup calls `exit(429)`, which a shell
   sees as 173.
6. **Output has no size bound.** Projection is `--jq`. There is no byte
   guard, and the envelope's `truncated` only means count equals limit.

Start-up is fine: 8–11 ms for `--version`, about 60 ms for a command with no
network call.

**Why build `dd` anyway.** agent-cli's advantage is what design.md measured:
search and 2 KB help, `--fields`, the 12 KB guard, metric series summarised
instead of dumped, dry-run and read-only enforced at the transport rather than
by command name, and exit codes that say what to do next. On top of that come
the joins with k8s. pup's advantage is breadth, so the plan is a hybrid:

- `dd` covers the roughly 20 calls agents make most while debugging and
  operating.
- For anything else, `agent-cli dd` and `agent-cli doctor dd` name pup, and
  recommend `PUP_READ_ONLY=1` because its guard has holes.
- Anyone already logged in to pup can hand agent-cli pup's OAuth token
  (`token_cmd = "pup auth token"`), so there is one login.
- Spec generation waits for PLAN's generic runner, and for trials showing
  agents need the long tail through agent-cli rather than pup.

## Approaches compared

| | (a) Wrap pup | (b) Call the REST API, hand-picked | (c) Generate from OpenAPI | (d) No domain; agents use pup |
|---|---|---|---|---|
| Install | pup on PATH (94 MB, from a brew tap or a tarball) | Nothing new: ureq and serde are already dependencies | Nothing new, plus an embedded catalogue built from 10 MB of YAML | pup |
| Version drift | About 5 releases a week; flags and the envelope change. Needs a pinned version and a contract test | 20 endpoints on the public v1/v2 API | Regenerate on each spec change; 621 of v2's operations are `x-unstable` | pup's problem |
| Secrets and hosts | pup holds the credentials; agent-cli can't enforce its host allowlist or redact pup's stderr | `Secret`, `host_under(api.<site>)` and redaction | As in (b) | pup's keychain and site trust prompt (good) |
| Output control | pup's JSON must be remapped, and its envelope depends on detecting the agent (`CLAUDECODE` is inherited), so every call needs `--no-agent` | Typed rows, `--fields`, the guard, series summaries | `--fields` and the guard; raw pointlists | `--jq`; no guard; 1 MB of help |
| Dry-run | Shows the argv, not the request | The exact request, via `ctx.write` | As in (b) | None |
| Read-only | An argv allowlist in the wrapper, on top of pup's leaky guard | Core's chokepoint; search POSTs go through `Request::query` | Needs a hand-kept list of POSTs that only read (below) | pup's write-verb list, bypassed as shown above |
| Latency | About 60 ms for pup to start, on every call | About 2 ms for agent-cli to start, plus the request | As in (b) | About 60 ms |
| Coverage | Whatever is wrapped | 20 commands | 1,831 operations (v1 235, 65 of them deprecated; v2 1,596) | 828 commands |
| Maintenance | Two contracts: pup's argv and its JSON | About 3k lines on stable endpoints | Generator work, then curation | None |
| k8s joins | Possible | Yes | Only through generic flags | No |
| **Verdict** | **No** | **Yes, now** | **Later, if trials ask for it** | **Yes, for the long tail** |

**The spec doesn't say which operations only read.** Logs, spans, events and
timeseries are all searched with a POST: v2 has 71 POSTs named `Search…`,
`List…`, `Query…` or `Aggregate…`, and v1 has 4. The spec has no `x-undo`, and
the client repo's `undo.json` can't stand in for it: it marks `MuteHost`
`safe` and `CreateEvent` `idempotent`, because "safe" there means the test
needs no cleanup. Credentials come as two schemes (a bearer token, or a pair
of key headers). So a generated `dd` needs a hand-kept list of the POSTs that
only read, plus cross-domain.md's table renaming `from`/`to` params to the
canonical flags.

## Commands

**Shared arguments**

- `[QUERY]`: Datadog search syntax for that product, ANDed with the typed
  filters.
- `--since`, `--until`: the core `When` type; help shows each default.
- `--cluster`, `--namespace`, `--pod`, `--deployment`: k8s filters (see K8s
  synergy).
- `--service`, `--env`: unified service tags. `--env` defaults to
  `[datadog] env`, otherwise `*`.
- `--limit` (int, default 50); `--tag str[]` (extra tags); `--full` (whole
  messages and all attributes).
- `--status str[]` is log or span status (`error`, `warn`, …). `--state
  str[]` is monitor or incident state, native values matched
  case-insensitively.

Every read goes through `ctx.read`; a search POST is sent as
`Request::query`.

| Command | Effect | Args (beyond the shared ones) | API call | Returns | Example |
|---|---|---|---|---|---|
| `dd log list [QUERY]` | Read | `--index str[]`; `--since` 15m; `--limit` at most 1000 | `POST /api/v2/logs/events/search`, sorted `-timestamp` | `[{time,status,service,host,message,error{kind,message},trace_id,pod,id}]` | `dd log list --service web --status error --since 1h --fields time,message,pod` |
| `dd log-count list [QUERY]` | Read | `--by str[]` facets (default `service`); `--since` 1h | `POST /api/v2/logs/analytics/aggregate`: count, grouped by the facets, sorted descending | `[{group{…},count}]` | `dd log-count list --status error --by service,kube_namespace --fields group,count` |
| `dd metric list [PREFIX]` | Read | `--since` 1h ("active since") | `GET /api/v1/metrics?from&tag_filter`, with the prefix filtered locally | `[name]` | `dd metric list kubernetes.memory --namespace web` |
| `dd metric get <QUERY>` | Read | `--points` 12 (0 for stats only); `--limit` 20 series; `--sort max\|avg\|last`; `--since` 1h | `GET /api/v1/query?from&to&query`. The string syntax takes arithmetic and `.rollup()`, so an agent can paste a monitor or dashboard query | `[{series,group{…},unit,min,max,avg,last,points[[t,v]],gaps}]` | `dd metric get 'avg:kubernetes.cpu.usage.total{*} by {pod_name}' --namespace web --since 3h --fields group,max,last` |
| `dd monitor list [QUERY]` | Read | `--state` (`Alert`, `Warn`, `No Data`, `OK`) | `GET /api/v1/monitor/search?query&per_page` | `[{id,name,state,type,priority,triggered,tags}]` | `dd monitor list --state Alert,Warn --tag team:web --fields id,name,state,triggered` |
| `dd monitor get <ID>` | Read | `--all-groups` (by default only groups that aren't OK) | `GET /api/v1/monitor/{id}?group_states=all&with_downtimes=true` | `{id,name,state,type,query,message,tags,triggered,groups[{name,state,triggered}],downtimes[{id,scope,end}],url}` | `dd monitor get 4711 --fields state,triggered,groups` |
| `dd downtime list` | Read | `--monitor int`; `--all` (include ended downtimes) | `GET /api/v2/downtime?current_only=true` | `[{id,status,monitor,scope,start,end,message}]` | `dd downtime list --monitor 4711 --fields id,status,end` |
| `dd downtime create` | **Destructive** | `--monitor int` or `--monitor-tag str[]` (one required); `--scope` (default `*`); **`--for dur`, required, at most 7d**; `--message` | `POST /api/v2/downtime` with `{monitor_identifier, scope, schedule{end}}` | `{id,status,monitor,scope,start,end,url}` | `dd downtime create --monitor 4711 --for 30m --message "deploy 1.42" --dry-run` |
| `dd downtime cancel <ID>` | Write | – | `DELETE /api/v2/downtime/{id}` (answers 204) | `{id,canceled}` | `dd downtime cancel 00000000-0000-0000-0000-000000000000 --dry-run` |
| `dd event list [QUERY]` | Read | `--since` 1h | `GET /api/v2/events?filter[query]&filter[from]&filter[to]&sort=-timestamp` | `[{time,title,source,message,service,pod,id}]` | `dd event list 'source:kubernetes' --namespace web --since 2h --fields time,title,pod` |
| `dd service list` | Read | – | `GET /api/v2/apm/services?filter[env]` (required; `*` means all) | `[name]` | `dd service list --env prod` |
| `dd service get <SERVICE>` | Read | `--since` 1h | Two `POST /api/v2/spans/analytics/aggregate` calls, for all entry spans and for `status:error`, each grouped by `resource_name` with count and pc50/95/99 of `@duration` | `{id,env,window,spans,errors,error_rate,p50_ms,p95_ms,p99_ms,resources[{resource,spans,errors,p95_ms}],sampled,url}` | `dd service get web --env prod --fields error_rate,p95_ms,resources` |
| `dd span list [QUERY]` | Read | `--min-duration dur`; `--since` 15m; `--limit` at most 1000 | `POST /api/v2/spans/events/search` | `[{time,trace_id,span_id,service,resource,operation,status,duration_ms,error{type,message},pod}]` | `dd span list --service web --status error --since 30m --fields time,resource,error,trace_id` |
| `dd incident list [QUERY]` | Read | `--state` (default `active,stable`); `--severity str[]` | `GET /api/v2/incidents/search?query&sort=-created` (at most 100 a page) | `[{id,title,state,severity,created,resolved}]` | `dd incident list --state active --fields id,title,severity` |
| `dd incident get <ID>` | Read | – | `GET /api/v2/incidents/{uuid}`; the public id is resolved by search and cached | `{id,uuid,title,state,severity,customer_impacted,created,detected,resolved,fields{…},url}` | `dd incident get 982 --fields title,state,fields` |
| `dd host list [QUERY]` | Read | `--cluster` only | `GET /api/v1/hosts?filter&count&sort_field=last_reported` | `[{id,up,last_reported,apps,cluster,muted}]` | `dd host list --cluster contoso-dev --fields id,up,last_reported` |
| `dd container list` | Read | – | `GET /api/v2/containers?filter[tags]&page[size]` | `[{id,state,host,image,started,pod}]` | `dd container list --namespace web --fields id,state,pod` |
| `dd slo list [QUERY]` | Read | – | `GET /api/v1/slo?query&tags_query&limit` | `[{id,name,type,target,timeframe,tags}]` | `dd slo list --tag team:web --fields id,name,target` |
| `dd slo get <ID>` | Read | `--since` defaults to the SLO's own timeframe | `GET /api/v1/slo/{id}`, then `/history?from_ts&to_ts` | `{id,name,target,timeframe,sli,budget_remaining,window,breaching[],url}` | `dd slo get 0c3fe5a1b2c34d5e --fields sli,target,budget_remaining` |
| `dd dashboard list [WORDS]` | Read | – | `GET /api/v1/dashboard`, all pages, cached for 10 min; titles matched locally | `[{id,title,modified,url}]` | `dd dashboard list kubernetes --fields id,title,url` |

**Deferred to pup** until a trial shows agents need them here: `incident
update` (Destructive, since resolving starts notifications); `monitor create`
and `monitor update` (Write); `trace get` (`span list 'trace_id:…'` covers
it); `dashboard get` (the widget queries); log patterns (pup reaches them only
through an internal endpoint).

## Search: keywords, synonyms, labeled queries

**Domain synonyms:** datadog → `dd`; apm, trace, tracing → `span`,
`service`; mute, silence, snooze, unmute, maintenance → `downtime`; alert,
alarm, page → `monitor` (cross-domain §5); outage, sev → `incident`;
timeseries, graph → `metric`.

**Collisions with other domains** need labeled queries for both readings
(cross-domain test 11): k8s's synonym `container → pod` against the `dd
container` resource, `k8s event` against `dd event`, and `k8s pod logs`
against `dd log list`. When an agent names Datadog, or asks about something
older than k8s's one-hour event retention, the query should land on `dd`.

| Command | Keywords | Labeled queries (at least two each) |
|---|---|---|
| log list | error, exception, stack, message | "datadog error logs for service web in the last hour"; "what did pod api-7d9f8 log before it died last night" |
| log-count list | count, top, breakdown, most | "which services log the most errors"; "count datadog logs by status in namespace web" |
| metric list | names, available, find metric | "which kubernetes memory metrics exist in datadog"; "find the metric name for request latency" |
| metric get | timeseries, graph, cpu, memory, rollup, trend | "cpu of pods in namespace web over the last 3 hours"; "is memory of deployment api growing" |
| monitor list | alert, alarm, firing, triggered | "which datadog monitors are alerting"; "list monitors for team web" |
| monitor get | why, groups, threshold, query | "why is monitor 4711 alerting"; "which pods triggered monitor 4711" |
| downtime list | muted, silenced, maintenance, scheduled | "is monitor 4711 muted"; "list active datadog downtimes" |
| downtime create | mute, silence, snooze, suppress | "mute monitor 4711 for 30 minutes"; "silence alerts during the deploy" |
| downtime cancel | unmute, unsilence, resume alerts | "unmute monitor 4711"; "end the datadog downtime early" |
| event list | deploy, change, kubernetes event, history | "datadog events for namespace web in the last 2 hours"; "what changed before the alert at 02:14" |
| service list | apm services, instrumented | "list apm services in prod"; "which services send traces" |
| service get | latency, p95, error rate, throughput, health | "error rate and p95 latency of service web"; "is the checkout service healthy" |
| span list | trace, slow request, exception, apm | "failing requests to web in the last 30 minutes"; "slow traces over 2 seconds for service api" |
| incident list | outage, sev, declared, active | "are there any active incidents"; "list sev-1 incidents this week" |
| incident get | root cause, commander, impact | "details of incident 982"; "who is commander of incident 982" |
| host list | node, machine, vm, agent reporting | "which datadog hosts are down"; "hosts reporting for cluster contoso-dev" |
| container list | docker, running, image | "containers datadog sees in namespace web"; "which image does datadog say the api container runs" |
| slo list | objective, reliability, target | "list slos for team web"; "which slos exist for checkout" |
| slo get | error budget, burn, compliance | "how much error budget is left for the web slo"; "is the availability slo breaching" |
| dashboard list | board, link, screen | "find the kubernetes dashboard link"; "which datadog dashboards mention checkout" |

## Config and auth

```toml
[datadog]
site = "datadoghq.eu"               # default datadoghq.com
env = "prod"                        # default for --env
token_env = "DD_ACCESS_TOKEN"       # default: a personal access token or an OAuth token
# token_cmd = "pup auth token"      # reuse pup's OAuth login (pup refreshes it)
# token_cmd = "pass show contoso/dd-pat"
# api_key_env = "DD_API_KEY"        # default for the fallback pair
# app_key_env = "DD_APP_KEY"        # or api_key_cmd / app_key_cmd
```

- **Credential order: a token first, then the key pair,** each from its
  environment variable before its command. A token goes in `Authorization:
  Bearer` through the existing `Mint` and is minted again once after a 401.
  A Datadog personal access token (PAT) is the recommended credential: it is
  scoped by default, expires (24 h to 1 y) and needs no API key.
- **Literal keys in the config are refused** (exit 3). sql allows `password`
  because its databases are throwaway; no Datadog key is.
- **Site.** Precedence: `AGENT_CLI_DATADOG_SITE`, `site`, `DD_SITE`, then
  `datadoghq.com`. It must be on the spec's list: `datadoghq.com`;
  `us3`/`us5`/`ap1`/`ap2`/`uk1` under `.datadoghq.com`; `datadoghq.eu`;
  `ddog-gov.com`; `us2.ddog-gov.com`. Anything else exits 3; custom hosts
  wait until someone needs one.
- **Host allowlist.** Every request goes to `https://api.<site>`, and
  `host_under(url, "api.<site>")` is checked before a credential is attached.
  Handlers build every URL themselves: paging uses `meta.page.after` and
  never follows `links.next`. Web links use the site table's app hosts
  (`app.datadoghq.com`, `app.datadoghq.eu`, `app.ddog-gov.com`, or else the
  site itself); they are parsed, never fetched.
- **The real guard** is a PAT or scoped application key with read scopes
  only (`logs_read_data`, `timeseries_query`, `metrics_read`,
  `monitors_read`, `events_read`, `apm_read`, `incident_read`, `hosts_read`,
  `slos_read`, `dashboards_read`), plus `monitors_downtime` only to allow
  muting. The spec lists the scopes per operation, so `doctor` can name the
  missing one on a 403.

**Core changes** (each small, each with a test):

1. **`Request::secret_header(name, Secret)`.** The key pair needs
   `DD-API-KEY` and `DD-APPLICATION-KEY`, but the transport only attaches
   `Authorization`. The dry-run plan prints the header names with `***`.
   About 25 lines, plus `FakeTransport`.
2. **A redaction gap.** `sensitive_key` and `TEXT_KEYS` match `api-key` but
   not `application-key`, so today a `DD-APPLICATION-KEY: …` line is masked
   only when its value is a known secret. Add `application-key`, `app-key`
   and `app_key` to both.
3. **`throttle_wait` reads `X-RateLimit-Reset`** (seconds). Datadog sends
   that and no `Retry-After`, so today every 429 waits the 30 s default.
   About 3 lines.
4. **A duration parser** for `--for` and `--min-duration`, next to `When`.
   Units: `ms`, `s`, `m`, `h`, `d`, `w`.
5. **Move sql's env/cmd/literal password resolution into core as
   `SecretSource`** once dd is its second user, instead of copying it.

**`doctor dd`** checks the config and site, reports which credential source
was found (never its value), and makes one live call: `GET /api/v1/validate`
for a key pair, or a 1-row `monitor/search` for a token. On a 403 it names
the missing scope. If pup is on PATH, it prints pup's version and "pup covers
the rest; run it with PUP_READ_ONLY=1". The overview status is `dd eu` or `dd
needs setup`, at most 12 characters, because cross-domain test 10 counts
every domain.

## Output control for big payloads

- **Log, span and event rows** are typed. The kube tags fold into `pod`, the
  k8s id `cluster/namespace/pod`. A message is cut to 300 characters plus
  `…(+N)`; `--full` restores whole messages and all attributes. The Explorer
  link goes to stderr as a note (`[open:
  https://app…/logs?query=…&from_ts=…&to_ts=…]`).
- **Span durations** are converted from nanoseconds to `duration_ms`. pup's
  own schema warns agents about nanoseconds; dd removes the trap.
  `--min-duration 2s` becomes `@duration:>2000000000`.
- **More results than printed.** The logs and spans APIs return a cursor, not
  a total, so the note says `[50 shown, more match; narrow QUERY or use dd
  log-count list]`. A `--limit` above 1000 (one API page) exits 2 and names
  `log-count`. Raw logs are never paged automatically.
- **Metrics.** Each series returns `min`, `max`, `avg`, `last`, and `points`
  downsampled to `--points` (default 12) by bucket mean, with nulls skipped
  and counted in `gaps`. Series are sorted by `--sort` (default `max`) and
  capped at `--limit 20`, with the note `[20 of 143 series; add a {tag} or
  --limit N]`. 20 series of 12 points come to about 5 KB.
- **`monitor get`** lists only groups that aren't OK unless `--all-groups`,
  and cuts its message to 500 characters.
- **Aggregates answer "how many".** `log-count list` and `service get`
  return counts that Datadog computes, not rows.
- **The 12 KB guard** still applies. Typical defaults stay under 5 KB; a log
  list of 50 long messages can reach it, and `--fields` is the way out.

## Safety and time budgets

- **17 commands are Read.** The POSTs among them (log search and aggregate,
  span search and aggregate) are sent as `Request::query`, and `ctx.read`
  refuses any other POST, so a handler can't write through the read path by
  mistake.
- **`downtime cancel` is Write:** it turns alerts back on.
- **`downtime create` is Destructive.** A mute can be undone; a missed page
  can't. It needs `--yes`, and `--for` is required and at most 7 days, so an
  agent can never leave a monitor muted indefinitely.
- **`--dry-run`** prints the request: `POST
  https://api.datadoghq.eu/api/v2/downtime`, the body, and every credential
  header as `***`. **`AGENT_CLI_READ_ONLY=1`** refuses both writes in core.
  No handler checks these flags itself.
- **Deadline:** 60 s by default, as for every command. Each command makes one
  call, except `service get` and `slo get` (two each), `incident get` by
  public id (two), and `dashboard list` (every page, then cached).
- **Long windows** are what make searches slow; pup's own anti-pattern list
  warns against `--from=30d`. When `log list` or `span list` gets `--since`
  longer than 1 day, a note suggests `log-count list`.
- **Throttling.** A 429 is waited out using `X-RateLimit-Reset`, only when
  the wait fits the deadline; otherwise it exits 124 with a hint.

## K8s synergy

**The tags Datadog attaches.** The Datadog Agent tags pod telemetry with
`kube_cluster_name` (from `DD_CLUSTER_NAME` or the cloud integration),
`kube_namespace`, `pod_name`, `kube_container_name`, `kube_deployment`,
`kube_stateful_set`, `kube_daemon_set`, `kube_job` and `kube_cronjob`. It
also adds `service`, `env` and `version` from the pod labels
`tags.datadoghq.com/{env,service,version}`.

**Filters.** Every command that takes k8s filters accepts all four, except
`host list` and `metric list`, which take only `--cluster`. They accept k8s
ids as k8s prints them (cross-domain §1); a ref that disagrees with a flag
exits 2.

| Flag | Accepts | Adds |
|---|---|---|
| `--cluster C` | a cluster name | `kube_cluster_name:C` |
| `--namespace N` | a namespace | `kube_namespace:N` |
| `--pod ID` | `C/N/P`, `N/P` or `P` | `pod_name:P`, plus any cluster and namespace parts |
| `--deployment ID` | the same shapes | `kube_deployment:…`, plus any cluster and namespace parts |

**How a filter is applied.** For logs, spans and events, the tags are
appended to the query as ANDed terms. For containers they go in
`filter[tags]`. In a metric query they go inside each `{…}` that follows a
metric name, replacing `*`, and `by {…}` is left alone. That is a brace
scanner with table tests, marked `ponytail:` because it doesn't handle braces
inside string literals.

**Rows point back at k8s.** Rows carry `pod` =
`kube_cluster_name/kube_namespace/pod_name`, which is the k8s pod id whenever
k8s accepts Datadog's cluster name. cross-domain.md makes k8s own the alias
table (scope name, kube context, AKS name), and the Datadog cluster name
belongs in it too. dd never reads `[[k8s.scope]]`, and neither crate depends
on the other.

**From k8s to dd: no code in k8s now.** After trials, cross-domain.md's
`next:` note could cover it, printed only when a `[datadog]` section exists
(a config check, not a network call). For a pod that isn't Ready:

```
next: agent-cli dd log list --pod prod/web/api-7d9f8-x2k4q --since 1h --fields time,message
```

The link is a string only, and test 6 parses it.

**Anything that needs both domains** is a composite in the crate of its
starting object, reaching dd through `Ctx::call` (cross-domain.md §4). For
example, `k8s deployment triage` would combine pod state, `dd log-count list`
and monitor state. There is never a Cargo dependency between dd and k8s.

**A conflict to settle.** Cross-domain test 5 makes `--cluster` and
`--namespace` default to the only configured k8s scope. In dd they are
optional filters, and leaving them out means all clusters. I recommend that
test 5 apply only to scope flags in the domain that owns the scopes, with
dd's filters exempt. The names stay canonical, because agents copy them from
k8s commands.

## Time windows (the core convention)

**Use cross-domain.md's `When` as it is:** `^\d+[smhdw]$`, `now-15m`, RFC
3339 with any offset, or `YYYY-MM-DD`. `--until` defaults to now. Output is
RFC 3339 UTC ending in `Z`; log and span times keep milliseconds. pup's long
forms (`5min`, `2hours`) and Unix timestamps are left out, and its
`--from`/`--to` are denied flag names whose error names `--since`/`--until`.
Add `min` only if trials show agents typing it.

**What dd adds**

- **Resolve once.** The window is resolved to absolute UTC once per
  invocation, then sent in the form each API wants: milliseconds for log,
  span and event `filter.from`/`to`; epoch seconds for v1 `query`, `metrics`,
  `hosts` and SLO history.
- **Show defaults.** When a default window applied, a note shows it: `[since
  15m ago (default): 13:45:00Z–14:00:00Z; --since 1h for more]`.
- **Default `--since`:** 15m for `log list` and `span list`; 1h for `metric
  get`, `event list`, `service get` and `log-count list`; the SLO's own
  timeframe for `slo get`.
- **Durations are not times.** `--for` (how long a downtime lasts) and
  `--min-duration` are durations, never named `since` or `until`
  (cross-domain test 1).

## Testing

**Offline, with no Datadog account**

- **Fixtures.** Tests run on `FakeTransport` with synthetic answers that use
  `contoso` names. The answers are built from the spec's property examples
  where it has them (7 of 7 properties in `LogAttributes`, 16 of 17 in
  `SpansAttributes`); timeseries and container answers have none, so they
  are hand-written. The client repos' recorded cassettes are not used: they
  record a real org.
- **Unit tests:** the duration parser; metric scope injection (`*`, existing
  tags, `by {}`, arithmetic, `.rollup()`, several queries in one string); k8s
  filter expansion and ref/flag conflicts; series stats with nulls, and
  downsampling; message cuts and nanoseconds to milliseconds; the site
  allowlist and credential order; resolving an incident's public id.
- **Secret tests.** The dry-run plan and every error from `downtime create`
  contain neither key value. `DD-APPLICATION-KEY` is masked by its name. A
  URL on another host is sent without a credential (`FakeTransport` records
  that). An unknown site exits 3 before any request.
- **Registry and search:** the invariants, dry-run tests for both writes,
  core's read-only refusal, and 40 labeled queries in `search.toml`,
  including the collision pairs.

**Live, on a sandbox**

- **Sandbox.** A 14-day Datadog trial org and a kind cluster running the
  Datadog Helm chart, with a demo app carrying the `tags.datadoghq.com`
  labels and one monitor to mute and unmute. Everything in it is synthetic,
  so fixtures captured there are safe to commit once scrubbed.
- **Smoke test.** A live test that is off by default
  (`AGENT_CLI_TEST_DATADOG=1`) runs the 17 reads with `--limit 1`, to catch
  API drift.
- **Agent trials.** Two arms per cross-domain.md: N (pup and kubectl) and A
  (agent-cli). Six tasks: the top error for a deployment in the last hour;
  why a monitor is alerting, and on which pods; p95 latency and error rate
  for a service; muting a monitor for a deploy (a dry-run first is
  expected); error counts by namespace; active incidents. dd earns its place
  if arm A beats arm N on at least half of the tasks.

## Effort

| Step | Days |
|---|---|
| Core: secret headers, the redaction fix, `X-RateLimit-Reset`, the duration parser, moving `SecretSource` | 1 |
| dd crate: config, auth, site table, doctor, status, k8s filters, metric scope injection | 2 |
| 20 commands with fixtures, dry-run tests and 40 labeled queries | 6 |
| Trial org and kind setup, live smoke test, agent trials, fixes | 2–3 |
| **Total** | **About 11–12 days** (about 3–3.5k lines, tests included) |

dd needs cross-domain.md's `When` type and canonical-flag checks first. It
doesn't need the k8s crate, so it can land any time after phase 1.

## Open questions

1. **Which Datadog org and site will this run against?** Can you create a
   PAT or a scoped application key? This decides between the token path and
   the key-pair path, and whether `doctor` can check scopes.
2. **Do you already use pup with an OAuth login?** If so,
   `token_cmd = "pup auth token"` gives one login and no long-lived key.
3. **The domain name.** `dd` matches `ado`, `kv` and `k8s`, and search maps
   "datadog" to it. Or do you prefer `datadog`?
4. **Muting.** Should agents be able to mute at all (Destructive, `--for` at
   most 7 days)? Or should dd be read-only in v1?
5. **Cluster names.** Does `kube_cluster_name` equal your k8s scope or AKS
   cluster names? If not, k8s's alias table needs a Datadog column
   (cross-domain question 5).
6. **dd's `--cluster` and `--namespace`.** Exempt them from cross-domain test
   5, as filters rather than scopes?
7. **How `service get` measures.** Sampled indexed spans: one fast,
   approximate call. Exact trace metrics (`trace.<op>.hits` and `.errors`)
   need the operation name: 2–3 calls. Which?
8. **Incident writes and `monitor create`.** Leave them in pup for v1? That
   is my recommendation.

## Sources

- **pup:** repository, README, `SKILL.md`, `docs/LLM_GUIDE.md`,
  `docs/COMMANDS.md`, `src/main.rs` (`is_write_command_name`; agent mode
  sets `auto_approve`), `src/config.rs`, `src/rate_limit.rs`
  (`EXIT_RATE_LIMITED = 429`): https://github.com/DataDog/pup. Releases and
  v1.23.6 assets: https://github.com/DataDog/pup/releases. Docs:
  https://docs.datadoghq.com/cli/
- **Spec:** the OpenAPI specs (`openapi.yaml` under `v1/` and `v2/`):
  https://github.com/DataDog/datadog-api-client-rust/tree/master/.generator/schemas.
  `undo.json`:
  https://github.com/DataDog/datadog-api-client-rust/blob/master/tests/scenarios/features/v2/undo.json
- **Datadog docs:** rate limits https://docs.datadoghq.com/api/latest/rate-limits/;
  sites https://docs.datadoghq.com/getting_started/site/; Kubernetes tags
  https://docs.datadoghq.com/containers/kubernetes/tag/; unified service
  tagging https://docs.datadoghq.com/getting_started/tagging/unified_service_tagging/;
  personal access tokens
  https://docs.datadoghq.com/account_management/personal-access-tokens/; API
  and application keys https://docs.datadoghq.com/account_management/api-app-keys/;
  authorization scopes https://docs.datadoghq.com/api/latest/scopes/;
  downtimes https://docs.datadoghq.com/api/latest/downtimes/
- **Local measurements (2026-09-29, Linux x86_64):** pup 1.23.6 help and
  schema sizes, start-up times and the read-only bypasses; the agent-cli
  release binary is 6.2 MB and prints the overview in 1–5 ms.
