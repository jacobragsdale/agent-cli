> History: the original plan (2026-09-29). For the current design read docs/explanation/design.md; for current work read TODO.md.

# Cross-domain synergy

How ado, kv, acr, aks, k8s, sql, airflow and dd work together without
coupling, per PLAN.md and the trial evidence in
`skillbook/skills/api-cli/references/design.md` ("design.md default N").
Most synergy comes from conventions and a few fields, not new machinery.

## Recommendations

**Build now** (core, before airflow and dd land; each is small and tested):

1. **One time window.** One core type parses `--since`/`--until`: `15m`,
   `2h`, `1d`, `1w`, `now-15m`, RFC 3339 or a plain date. Every printed
   timestamp is RFC 3339 UTC ending in `Z`, so it pastes back into a flag.
2. **The id is the ref.** A row's `id` is exactly what its resource's `get`
   accepts, with no other flags. A field naming another domain's thing holds
   that thing's `id`. Consumers also accept the pieces as flags, and the web
   URL. No ref type, no URI scheme.
3. **Canonical flag names**, enforced by `check_registry`: `--since --until
   --limit --tail --cluster --namespace --conn`. Synonyms (`--from`,
   `--after`, `--count`, `--top`, `--ns`, `--context`, …) are rejected with
   the canonical name in the error.
4. **Printed command lines must parse.** Every fixture test parses each
   `agent-cli …` in a `hint:` or `next:` line against the registry.
5. **Two checks:** the overview budget with every domain configured (only an
   empty config is checked today), and domain synonyms colliding with another
   domain's resource.

**Build with each domain** (fields and one command; no cross-domain code):

- **k8s:** new `k8s deployment list` (images, `updated`, `--since`); pod rows
  print `containers[].image` with the running digest; `pod get` returns
  `secret_refs` with CSI classes resolved to kv ids; `--cluster` accepts the
  scope name, kube context or AKS name.
- **acr:** `manifest get` takes one image reference and returns OCI `labels`.
- **ado:** `run list --commit`, `pr list --commit`; `run get` returns
  `commit`, `pr`, `workitems`; ids also accept `#123`, `AB#123` and web URLs.
- **airflow:** task-instance id `dag/run/task[/try]`; rows print `start`,
  `end`, `hostname`.
- **kv:** ids are `vault/name`; the secret URI is accepted too.
- **dd:** `--since`/`--until`; monitor rows print when they triggered and
  their group tags.
- **Scope helper** ("the only scope, else exit 2 listing them"): write it
  when k8s becomes its second user, and switch sql `--conn` to it then.

**Build after trials:** `next:` notes on stderr for results that exit 0 in a
failed or attention state; `Ctx::call` and the first composite (`k8s
deployment trace`), later `airflow run triage`; time arithmetic such as
`--since 2026-09-29T02:14Z-2h`; a `See:` line in help.

**Don't build:** `next` or `_links` fields in stdout; a typed `Ref` struct,
URI scheme or global `resolve` command; sticky context (`agent-cli use
prod`); a cross-domain identity map; flag alias tables; a recipe DSL; a
composite crate that depends on domain crates; a `timeline` before checking
what Datadog already ingests; "related commands" in search output; 1:1
kubectl or az verbs with no bounds or joins (`exec`, `apply`,
`port-forward`, a generic `resource get`, `--follow`); state values
normalized across domains.

## 1. Shared references

**The rule:** producers print native identifiers; consumers accept every
native spelling they are likely to be handed. So no domain reads another
domain's config or code. k8s owns the cluster alias table (scope name, kube
context, AKS name), because Datadog tags pods with `kube_cluster_name` and
aks prints AKS names; neither should need agent-cli's scope names.

| Thing | `id` (what `get` / `logs` takes) | Also accepted |
|---|---|---|
| ADO work item / PR / run | `1207` / `431` / `8812` | `#1207`, `AB#1207`, the `dev.azure.com` URL |
| k8s pod, deployment | `prod/web/api-7d9f8-x2k4q` (cluster/namespace/name) | `web/api-…` or `api-…` with defaulted or given `--cluster`/`--namespace` |
| Image | `contosoacr.azurecr.io/api@sha256:9f2c…` or `…/api:1.42.0` | `api:1.42.0` when one registry is configured |
| kv secret | `kv-contoso-prod/db-password` | the secret URI `https://kv-contoso-prod.vault.azure.net/secrets/db-password[/v]`; a bare name when unique |
| Airflow run / task instance | `etl_nightly/scheduled__2026-09-28T00:00:00+00:00`, then `…/load_orders[/2]` | the Airflow UI URL |
| dd monitor / incident | `4711` / `982` | the Datadog app URL |
| sql object | `dbo.customers` with `--conn` | unchanged; nothing consumes it yet |

- **`/` is a safe separator:** no name above can contain one (k8s DNS names,
  Key Vault names, Airflow's default `allowed_run_id_pattern`; the airflow
  plan confirms this and picks a map-index spelling).
- **Scope in the ref** beats the default. A ref and a flag that disagree is
  exit 2; the CLI never guesses.
- **URLs** are parsed, never fetched, and must name a configured org or host,
  else exit 2 naming the configured one. Tokens still go only to allowlisted
  hosts.
- **Evidence:** in design.md round 2 agents tried positional values first,
  which gave default 6 (bare values fill path parameters). Agents also copy
  what help shows (default 3). An `id` that is the ref is what they already
  copy.

**Where the next step goes:**

| Option | Cost | Verdict |
|---|---|---|
| Neither: the agent builds refs from fields and flags | Usage errors; Haiku was the weak spot in design.md round 2 | No |
| Typed ref objects `{"kind":"k8s.pod","cluster":…}` | 40–80 B per ref, and the agent still maps kind to command | No |
| `id` strings | About 0 B: the id replaces the cluster and namespace columns | **Yes** |
| `next: [...]` in stdout JSON | Dropped by `--fields`, which agents use on almost every first data call (default 3); ~80 B per row eats the 12 KB guard; breaks "stdout is data" | No |
| A `next:` line on stderr (`ctx.note`) | ~20 tokens, only when printed; survives `--fields` and the guard | Yes, restricted |

**`next:` rules:** at most one line; only on a single-object result in an
attention state (failed run or task, pod not Ready, monitor in Alert); it
names a Read command and must parse (test 6). **Now** where the command
already exits non-zero, where it is just the existing `hint:` (default 7:
errors carry the next step), e.g. `ado run wait` on a failed run pointing at
`ado run logs`. **Trial first** for results that exit 0:

```
$ agent-cli airflow run get etl_nightly/scheduled__2026-09-28T00:00:00+00:00 --fields id,state,failed
{"id":"etl_nightly/scheduled__2026-09-28T00:00:00+00:00","state":"failed","failed":["etl_nightly/scheduled__2026-09-28T00:00:00+00:00/load_orders/2"]}
next: agent-cli airflow task logs etl_nightly/scheduled__2026-09-28T00:00:00+00:00/load_orders/2 --tail 200     (stderr)
```

## 2. Conventions that make chaining work

**Time window.**

- A core type `When` gets a new arg kind `time`. It accepts `^\d+[smhdw]$`,
  the same with `now-` (a Datadog and Grafana habit), RFC 3339 with any
  offset, and `YYYY-MM-DD` (00:00 UTC), resolved to UTC at parse time.
  `--until` defaults to now. Help names the field filtered on.
- A time-series command (dd logs) needs a window; its default is a clap
  default (shown in help) and a note says when it applied, like `[50 of 312;
  --limit N]`: `[since 1h ago (default); --since 1d for more]`.
- Output is RFC 3339 UTC with `Z` in whole seconds (log lines keep ms).
  Relative forms and the overview's `Now:` line spare the agent date
  arithmetic, the likeliest source of silent errors.

**Identity.** `@me` means "the authenticated user of this domain": ado
resolves it via connectionData, airflow via its auth user, dd via
`current_user`, each cached in core's cache. Identity flags (`--assignee`,
`--author`, `--reviewer`, `--creator`, `--owner`) say so in their help (test
4). No ADO-user-to-Datadog-user mapping; nothing asks for one.

**Scope defaults.** One rule for every scope flag (`--cluster`,
`--namespace`, `--conn`, and whatever the airflow and dd plans pick for
instance or site): exactly one configured scope is the default; otherwise
exit 2 with `configured: a, b`, before any network call. PLAN.md says this for
k8s, but sql `--conn` is required today; changing it is one line once the
helper exists. No sticky contexts: every call is self-describing, so a
transcript is its own reproduction.

**Positionals.**

- One main positional, taking the `id` form from section 1, plus web URLs
  and ADO's `#`/`AB#` forms.
- A shared flag name whose values differ by domain (`--state`, `--type`) uses
  `String` with a `PossibleValuesParser`, not a `ValueEnum`. Its kind stays
  `str` and help still lists the values; a `ValueEnum` has kind `enum` and
  fails the one-type-per-flag check against ado's free-text `--state`.
- State values stay native (`failed`, `Failed`, `Alert`), matched
  case-insensitively, never normalized.

**Bounds.** Every `list` takes `--limit` (int, default 50); config-only lists
such as `sql connection list` are exempt by name. Every `logs` takes `--tail`
(default 200) and never `--follow`.

## 3. Real cross-system questions

**Retention changes the chains.** Kubernetes keeps events 1 h by default
(`--event-ttl`), and KubernetesPodOperator deletes its pod on finish (default
`on_finish_action`). For "last night", Datadog is the only lasting source:
the chain is airflow → dd, not airflow → k8s. The k8s hop only helps with
failures that are still live.

**Q1. What's deployed in prod, and which build, PR and work items produced
it?** Today the chain breaks after the pods: the plan has no `deployment
list`, `manifest get` has no build link, and the agent would guess whether
the tag is a build number. With the additions:

```
agent-cli k8s deployment list --cluster prod --namespace web --fields id,images,updated
agent-cli acr manifest get contosoacr.azurecr.io/api:1.42.0 --fields digest,created,labels
  {"digest":"sha256:9f2c…","created":"2026-09-28T21:01:40Z",
   "labels":{"org.opencontainers.image.revision":"4be1c0d…","org.opencontainers.image.source":"…/_git/api"}}
agent-cli ado run list --commit 4be1c0d --since 7d --fields id,pipeline,result,finished
agent-cli ado run get 8812 --fields id,commit,pr,workitems
```

Smallest additions: `k8s deployment list`; OCI `labels` in `acr manifest
get` (one more GET, for the config blob); `ado run list --commit` (the Builds
API has no commit filter, so it pages the runs in the window and filters);
`commit`, `pr`, `workitems` in `ado run get` (the build work-items API). It
works only if pipelines stamp images (open question 2).

**Q2. Why did last night's DAG fail?**

```
agent-cli airflow run list --dag etl_nightly --state failed --since 1d --fields id,state,end
agent-cli airflow task list etl_nightly/scheduled__… --state failed --fields id,start,end,hostname
  {"id":"etl_nightly/scheduled__…/load_orders/2","start":"2026-09-28T00:41:07Z",
   "end":"2026-09-28T00:43:55Z","hostname":"etl-nightly-load-orders-q8x1"}
agent-cli airflow task logs etl_nightly/scheduled__…/load_orders/2 --tail 200
agent-cli dd logs list 'pod_name:etl-nightly-load-orders-q8x1 status:error' \
    --since 2026-09-28T00:41:07Z --until 2026-09-28T00:43:55Z --fields time,message
```

Smallest additions: task-instance rows carry `start`, `end`, `hostname`, and
`--since`/`--until` accept printed times as they are. No cross-domain command;
this is where the shared window pays off most.

**Q3. Is this incident caused by a deploy?**

```
agent-cli dd monitor get 4711 --fields id,state,triggered,groups
agent-cli k8s deployment list --cluster prod --namespace web --since 2026-09-29T00:14:00Z --fields id,updated,images
agent-cli ado run list --pipeline api-cd --since 2026-09-29T00:14:00Z --until 2026-09-29T02:14:00Z --fields id,result,finished
```

Gaps: `k8s deployment list --since` (the Q1 command) and `--since`/`--until`
on `ado run list`. The agent works out T minus 2 h itself; add `T-2h` syntax
only if trials show arithmetic mistakes. If Datadog already ingests deploy
and k8s events, `dd event list --since …` answers this alone (open question
3).

**Q4. Which secrets does this pod use, and when do they expire?**

```
agent-cli k8s pod get prod/web/api-7d9f8-x2k4q --fields id,secret_refs
  {"secret_refs":[{"via":"env","secret":"prod/web/api-env","keys":["DB_USER"]},
                  {"via":"csi","class":"api-kv","kv":["kv-contoso-prod/db-password","kv-contoso-prod/api-key"]}]}
agent-cli kv secret list kv-contoso-prod/db-password kv-contoso-prod/api-key --fields id,enabled,expires
```

Smallest additions: `secret_refs` on `pod get`, resolving the
SecretProviderClass inline (one extra kubectl read: the join agents can't
cheaply do), and `kv secret list` treating a `vault/name` word as exact.

## 4. Composite commands ("trace", "triage")

**The rule.** Build a composite only when all four hold: (1) in arm A
(conventions only) the same chain of 3+ calls appears in at least 2 of 3
transcripts for a task class; (2) the chain needs no judgment between steps;
(3) an A/B trial cuts median tokens by at least 25%, with correctness no
worse on Sonnet or Haiku; (4) it is read-only. Fields that shorten the chain
come first: Q1–Q4 drop to 2–4 calls without any composite.

**Where they live.**

- **Rejected:** a `trace` crate depending on domain crates (it couples to
  their internals, and agents won't guess a domain named after a verb), and a
  recipe DSL (a language to maintain).
- **Proposed:** a composite is an ordinary `command!` in the crate of its
  starting object (`k8s deployment trace` lives in k8s). It reaches other
  domains only by path, through a new `Ctx::call(path, argv) ->
  Result<Value>` (about 40 lines in core). `Ctx::call` resolves the path in
  the registry, **refuses any effect but Read**, parses with `parse_leaf`,
  runs on the same `Ctx` (shared deadline, read-only mode and transport, so
  fixture tests work unchanged) and returns the value unprinted. The coupling
  is the public CLI contract an agent uses, guarded by the handoff tests; no
  Cargo edge between domains.

**Budgets.** One `--timeout` for the whole composite; steps run in sequence
until measured. A step that exits 3 or 124 becomes `{"skipped":"dd: needs
setup"}` and the rest still returns. Steps are projected to fixed fields, logs
use `--tail 50`, and the 12 KB guard applies as usual.

**Candidates:**

1. **`k8s deployment trace <id>`:** deployment → image → manifest labels →
   run → PR → work items. Deterministic, 4–5 calls; first to build, **after**
   the trial shows arm A agents taking 4+ calls or erring (tag instead of
   digest).

   ```
   {"id":"prod/web/api","image":"contosoacr.azurecr.io/api@sha256:9f2c…","run":{"id":8812,"pipeline":"api-ci","commit":"4be1c0d","finished":"2026-09-28T21:03:11Z"},"pr":{"id":431,"title":"Retry on 429"},"workitems":[{"id":1207,"title":"Throttle handling","state":"Resolved"}],"elapsed_ms":2140}
   ```

2. **`airflow run triage <id>`:** the failed tasks (at most 3), a 50-line log
   tail for each, and dd error logs in each task's window. Later, if Q2
   transcripts converge.
3. **A cross-domain `timeline` of changes: probably never.** It needs a new
   built-in word, and Datadog's event stream may already be that timeline.

`trace` and `triage` would be deliberate `VERBS` edits.

## 5. Search and discovery across domains

`search::expand` already applies every domain's synonyms globally, and phrase
keys work. What's missing is collision handling: a literal term and a domain
synonym both weigh 1.0, so ado's `task → workitem` ties airflow's `task`
resource.

| Word | Meanings | Plan |
|---|---|---|
| task | ADO Task work item; Airflow task instance | Keep both; labeled queries for each reading |
| run / pipeline | ado run; airflow DAG run; the `sql query run` verb | airflow keys `dag run`, `workflow`, `data pipeline`; the domain word decides |
| job | ADO job, k8s Job, Airflow task | No synonym anywhere until a trial misses one |
| deploy / deployment | k8s deployment; ADO deploy stage or approval; DD deployments | k8s owns the resource; "deploy" stays an ado keyword |
| container | pod; image | Phrases: `container image` → acr, `container logs` → k8s pod logs |
| image, tag, digest / alert, alarm, page / build, ci | acr / dd monitor, incident / ado run | Single-domain synonyms |

`check_registry` reports a synonym key equal to another domain's resource,
unless allowlisted with labeled queries for both readings; if the gates still
fail, lower the domain-synonym weight to 0.9 and re-measure. Cross-domain
intents ("which build made prod's image") go into trials, not `search.toml`:
they have no single right first command. No "related commands" in search (it
lengthens every hit list, unproven) and no cross-domain overview line (test
10 keeps it under 1 KB).

## 6. Push back

- **The value is in joins and bounds, not 1:1 verbs.** Agents know kubectl
  and az well. A k8s command earns its place only if it bounds output, adds
  safety, or joins sources (pod → CSI → kv). That rules out `exec`, `apply`,
  `port-forward` and a generic `resource get`.
- **Too many domains.** `aks` has two commands, and `k8s context list`
  overlaps them: three names for one cluster. Fold them into `k8s cluster
  list` (one row per scope with its context, AKS name and namespaces, plus
  discovered AKS clusters not yet configured) and `k8s cluster connect` (`az
  aks get-credentials`, as today). One domain and one command fewer, and the
  list doubles as the alias table (open question 5).
- **A new domain earns its place when all four hold:** (1) 6+ real,
  recurring tasks; (2) a naive-arm trial with the tool agents would otherwise
  use (kubectl, az, curl, pup) shows a loss agent-cli fixes: wrong answers,
  outputs over 30 KB, 2× the calls, or an unguarded write; (3) its auth fits
  one config section plus `doctor`; (4) it has 5+ commands. With fewer, it is
  a resource of an existing domain.
- **Wrapping a CLI versus calling the API.** A wrapper buys auth and
  transport, not modelling: its output still needs remapping to ids, UTC and
  bounds, plus a process start per call and a version to track. The dd plan
  decides; the conventions above bind it either way.
- **The growth path passes the same invariants.** Datadog's API uses
  `from`/`to` epoch seconds and ADO's `minTime`/`maxTime`, so the spec
  generator needs a small table renaming time, limit and identity params to
  the canonical flags. Without it `check_registry` rejects them, as intended.

## Invariant tests

**R** goes in `check_registry`; **T** goes in `testing::run`, so every
fixture test gets it for free; **E** is in `crates/cli/tests`.

| # | Test | Where | Asserts |
|---|---|---|---|
| 1 | `time_flags_are_since_and_until_and_use_the_core_parser` | R | Any arg of kind `time` is named `since`/`until`, and vice versa |
| 2 | `no_flag_is_a_synonym_of_a_canonical_one` | R | A deny table (`from after start to before end count top max ns context connection`) is never declared; the message names the canonical flag |
| 3 | `lists_take_limit_and_logs_take_tail` | R | `list` has `--limit` int default 50 (config-only exempt); `logs` has `--tail` and no `--follow`/`--watch` |
| 4 | `identity_flags_accept_at_me` | R | Help for flags in the identity table mentions `@me` |
| 5 | `a_scope_flag_defaults_to_the_only_scope_or_asks` | E | 2-scope fixture config, no flag: exit 2, both names in stderr, `FakeTransport::sent()` empty. 1 scope: no scope error |
| 6 | `every_printed_command_line_parses` | T | Each `agent-cli …` in `hint:`/`next:` resolves to a command and passes `parse_leaf` |
| 7 | `every_printed_timestamp_is_utc` | T | Any string that parses as a date-time ends in `Z`; data passthrough (`sql query run`) is exempt |
| 8 | `ids_round_trip_to_get` | T helper | A list fixture's `id`, passed to the sibling `get`, parses, needs no scope flag, and sends the expected request |
| 9 | `cross_domain_handoffs_resolve` | E `handoff.rs` | A table of (producer fixture field → consumer argv → expected request), e.g. a pod's `containers[0].image` → `acr manifest get` hits `contosoacr.azurecr.io/v2/api/manifests/sha256:…` |
| 10 | `the_overview_fits_with_every_domain_configured` | R | At most 1024 B with a full fixture config. Only `Config::empty()` is checked today; 8 status strings of 40 chars would overflow |
| 11 | `domain_synonyms_do_not_shadow_other_resources` | R | See section 5 |
| 12 | `composites_only_call_read_commands` (later) | unit | `Ctx::call` on a Write, Destructive or Reveal path errors; every `trace`/`triage` command is Read |

## Validating with agent trials

Use design.md's method (fresh Sonnet 5.5 and Haiku 4.5 subagents, a prompt
naming only the tool and task, the argv/exit/size shim, tokens, calls, time
and correctness per task) with **3 runs per cell** (design.md ran one and
flagged the noise) and **thresholds fixed before the run**.

**Tasks** (ground truth seeded or checked by hand), plus two single-domain
control tasks that catch hints sending agents on detours:

- **X1:** "Which work items shipped in the api image running in prod?"
- **X2:** "etl_nightly failed last night. What is the first error, and where?"
- **X3:** "Monitor 4711 alerted around 02:14 UTC. Did anything deploy to its
  service in the 2 h before?"
- **X4:** "Which Key Vault secrets does the api pod in prod use; does any
  expire within 30 days?"
- **X5:** "Is the image from `https://dev.azure.com/contoso/web/_build/results?buildId=8812` running in prod?"
- **X6:** "Is api in dev the same image as api in prod?" (digests, not tags)

**Arms:** **N**, the raw tools (kubectl, az, curl plus REST, pup), which is
the domain-earns-its-place check; **A**, the build-now conventions; **B**, A
plus `next:` notes; **C**, A plus the composite, for X1 and X2 only.

**Metrics** beyond design.md's: discovery calls (search, help, listings),
exit-2 count, ref-assembly errors (a usage error about an id or scope),
`next:` follow rate (the next argv equals the hint), stdout bytes.

**Decision rules:**

| Feature | Keep it if |
|---|---|
| `next:` notes | Median calls on targeted tasks drop by at least 1, the follow rate is at least 50%, and control tasks cost no more |
| Composite | It meets rule 1 in section 4 in arm A, and arm C wins by at least 25% of tokens at equal correctness |
| A domain | Arm A beats arm N on at least half its tasks, by 20% of tokens or on correctness |

Every miss becomes a fix, a test or a labeled query, as PLAN.md says.

## Decided (2026-09-29)

- **Images are tagged with the git tag, and pushing that tag triggers the ADO
  build.** The deploy trace is therefore:
  1. `k8s deployment list` gives the image tag.
  2. `ado run list --branch refs/tags/<tag>` finds the build.
  3. `ado run get` gives the commit, PR and work items.

  OCI labels on `acr manifest get` drop out of "build now". `ado run list
  --branch` must accept both a bare tag and a `refs/tags/…` ref.
- **Trials use local fixtures only.** A `fixtures` cargo feature replays
  recorded HTTP answers, alongside the fake `az`/`kubectl`/`kubelogin`
  scripts. One seeded contoso world is kept consistent across domains, so
  cross-domain tasks can be tried.
- **`aks` stays a separate domain,** because it calls Azure while `k8s` wraps
  kubectl. `aks cluster list` prints the `k8s` scope name, so the id is the
  ref.
- **Airflow runs on AKS** with the Helm chart and KubernetesExecutor, as dev,
  qa and prod instances (see `airflow.md`).

## Open questions

1. **Trial environment.** Your real environment under
   `AGENT_CLI_READ_ONLY=1`, with transcripts kept private and only metrics
   committed (realistic, cheap)? Or a seeded sandbox of kind, local Airflow,
   a personal ADO org, ACR Basic and a Datadog trial (reproducible, days of
   setup)?
2. **Do the pipelines stamp images with `org.opencontainers.image.revision`,
   or tag them with the build id?** Q1's chain, and whether `trace` is
   possible at all, depend on it.
3. **Does Datadog already ingest k8s events and ADO deploy events?** If yes,
   Q3 needs no k8s or ado hop, and `timeline` is dead.
4. **Which Airflow executor or operator runs the tasks** (Kubernetes, KPO,
   Celery)? That decides whether `hostname` is a pod name worth printing.
5. **Fold `aks` into `k8s cluster list/connect`** and drop `k8s context
   list`? And can `[[k8s.scope]] name` default to the AKS cluster name, so
   Datadog's `kube_cluster_name` matches with no alias?
6. **Should sql `--conn` default to the only connection,** matching k8s? Or
   stay required everywhere as a safety habit, with k8s matching that?
