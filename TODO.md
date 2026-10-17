# TODO

Open work only. What is built is in the code, the crate cards and
`docs/explanation/design.md`.

## Live runs

- [ ] Work through `docs/first-live-run.md`: sql, ado (a sandbox organization, 2026-10-02) and airflow (local 2.9.3 and 3.3.2, `scripts/airflow-up.sh`, 2026-10-03) have run against real services; confluence's reads have run against a public site; kv, acr, aks, aisearch, arm, net, lake, k8s and dd have not
- [ ] Then the one-line global CLAUDE.md note that agent-cli exists (the README gives the line)

## Confluence and AI Search (built 2026-10-07 from the API specs; no live service yet)

- [ ] confluence, live: a real Cloud site with a token (a free site works: 10 users, API tokens). Every read ran anonymously against a public Cloud site; the writes have run only against recordings: create, update with `--if-version`, section splices and the loss gate, inline comments and resolving one (does it need the body?), upload, move, labels. Record the v2 400 text for a duplicate title and for malformed storage, the 409 of a space that requires approval, and `users-bulk`'s cap (chunked at 100). Then a trial against curl (and `twg`, if its OAuth works in WSL): the plan's six tasks were find a runbook, what changed this week, open inline comments, save an attachment, publish a page, add a section without touching its macros
- [ ] confluence, if work turns out to run Data Center: a v1 adapter (`/rest/api/content` with `expand=body.storage,version,space,ancestors`, `child/page`, `/rest/experimental/content/{id}/version`, `child/attachment`, `/rest/api/space`), a context path in the base, `username`/`userKey` people, a PAT as Bearer, and a doctor that treats an anonymous `user/current` as a failure. Live tests on `atlassian/confluence` with Atlassian's 3-hour timebomb licence
- [ ] aisearch, live: a Free service (one per subscription, 50 MB, 3 indexes and indexers) and an opt-in `AGENT_CLI_TEST_AISEARCH` suite that makes its own index. Settle whether Resource Graph rows carry `endpoint` and `authOptions` (today an ARM GET fills them), 409 or 429 for a run already going, whether `"<unchanged>"` keeps a vectorizer's `apiKey` on an index PUT, and whether `If-None-Match: *` holds on index PUT
- [ ] aisearch, when asked: knowledge bases (agentic retrieval, GA but extractive only), facets (`facet list INDEX --field F`), the analyzer, a bare alias in the bare-name lookup, query keys for identities that only search, sovereign clouds

## k8s and Azure at work (built 2026-10-15 against the contoso world; no live cluster or subscription yet)

Each `VERIFY(work)` in `crates/k8s` and `crates/azure` is a line below; settle it against the work AKS and subscription, then drop the marker.

- [ ] First, the questions that decide what to build next: which CNI the AKS runs (Azure CNI overlay, or Cilium, whose policies go through `k8s object`) and which ingress (AGIC, app routing or Gateway API), which decides whether `net appgw`, `k8s ingress` or Gateway objects need more; whether its API server is private (then `aks command run` is the way in from WSL and moves up); whether `resourcechanges` reaches back 14 days on the work subscriptions (`arm change list`); and which api-version `arm resource get` should read a preview-only type with (it takes the newest preview, `arm/resource/get.rs`)
- [ ] k8s (`crates/k8s/src`): whether `--request-timeout=10s` cuts a `pod run` exec that runs longer (`pod/run.rs`); a real API server's wording for failing `/readyz` checks (`context/get.rs`); how long a joined `get` takes before adding any parallelism
- [ ] aks (`crates/azure/src/aks`): that a pool PUT of its own GET less the read-only fields is taken as is (`nodepool/scale.rs`); in `command/run.rs`, that AKS runs the command through a shell, whether a cluster with local accounts off needs `clusterToken`, the 202 and `commandResults` Location answering 202 then 200, and how much output AKS keeps and which end it cuts
- [ ] arm (`crates/azure/src/arm`): that `resourcechanges` rows carry `resourceGroup` (`change/list.rs`); whether management-group assignments show up in a subscription's `authorizationresources`, and that built-in role definitions are there by GUID (`role/mod.rs`); that built-in policy definitions are in `policyresources` (`policy/list.rs`); that a provider lists a type nested deeper than two by its full path (`resource/get.rs`); a service principal's `--caller @me` (its app id against the log's object id), and that `resourceUri eq` matches a resource's role assignments (`activity/list.rs`); whether a managed disk has an IOPS-consumed percentage (`fixtures/world/facts/arm.md` says no)
- [ ] net (`crates/azure/src/net`): that usages' `limit` already leaves out Azure's five reserved addresses (`mod.rs`); that Resource Graph holds private zones' vnet links as resources (`zone/list.rs`); that `backendhealth` takes an empty body and answers 202 with a Location (`appgw/get.rs`); that AKS tags a LoadBalancer service's public IP `k8s-azure-service` (`ip/mod.rs`); that AKS tags its scale sets `aks-managed-poolName` (`subnet/get.rs`); in `nic/get.rs`, that a scale set instance's NIC answers at `2018-10-01` only, the effective rules' and routes' 202 and Location, and whether Azure answers either for an AKS node's NIC at all (today the subnet's NSG and route table stand in, with a note)
- [ ] kv (`crates/azure/src/kv`): that an imported certificate's policy holds its own subject and SANs (`certificate/mod.rs`); that a key with no rotation policy answers 404, not an empty policy, and that Key Vault Crypto User may read it (`key/get.rs`)
- [ ] acr (`crates/azure/src/acr`): the ACR Tasks runs `$filter`'s field names and that runs come newest first (`run/list.rs`); that a run's log blob is plain UTF-8, not gzipped (`run/logs.rs`)

## lake (built 2026-10-16 against the contoso world; no live account yet)

- [ ] Step 0's trial, not run: agents with `az` against a sandbox account seeded with the world's files (`fixtures/world/files/lake/`), on the tasks in `crates/cli/tests/world_lake.rs` (did today's partition land, which days are missing, the ledger's schema and rows, Airflow's 403). Count calls, tokens and wrong answers; lake keeps its place if it wins at least half
- [ ] Synapse at work (`sql` with `auth = "az"`): a first connection to the serverless endpoint (TLS 1.2, the FedAuth token login tiberius sends); which catalog views serverless answers for `sql object list`, `object get` and `schema list` (`crates/sql/src/catalog.rs`), and what `sql query run` should say for Synapse's "File cannot be opened" (the next step is `lake access get PATH --principal @me`); whether the BULK path needs percent-encoding for a name with spaces
- [ ] Open questions from the plan: whether every container holds the same data domains as top-level folders; whether the lake is reachable only through private endpoints (WSL resolving `privatelink.dfs.core.windows.net`)
- [ ] `VERIFY(work)` in `crates/azure/src/lake`: that the accounts take `x-ms-version` 2025-01-05 (`mod.rs`); that `HEAD /{container}/` answers for the root directory (`mod.rs`); that the az login's Graph token holds Directory.Read.All for another principal's groups (`/me` needs only User.Read), else group ACL entries never count (`access/get.rs`); where `authorizationresources` keeps a custom role's `dataActions`, which `access get` ignores today (`access/get.rs`)
- [ ] The world: Airflow's task log printing the 403 with the `abfss://` URL (a failed run in `http/airflow.json`, which ripples through its DAG and run lists), and `net endpoint get pe-contosolake-dfs` with its NIC and zone record
- [ ] When asked: `lake path delete` (Destructive); airflow and controlm notes naming `lake path get` when a log holds an `abfss://` URL

## entra (built 2026-10-16 against the contoso world; no live tenant yet)

- [ ] Step 0's trial, not run: agents with `az ad` and `az rest` against a P1 sandbox tenant seeded with the world (a one-day secret makes the expired one), on six tasks: credentials expired or ending within 30 days and their owners; sg-contoso-platform's members, nested, and how; the names of every principal with a role on the subscription; what `…a1f2` is and was granted; why contoso-ado-deploy cannot sign in, and with which secret; who added that secret, and sam to the oncall group. entra keeps its place if it wins at least half
- [ ] Open questions from the plan: whether the work login is a member, not a guest, and whether conditional access refuses az's Graph token; whether the work tenant is on P1 or P2 and the login holds Reports Reader, Security Reader or Global Reader (else `signin list` and `audit list` cannot run there); whether the work's ADO service connections use secrets or workload identity federation
- [ ] `VERIFY(work)` in `crates/azure/src/entra`: that a member's az token reads `getByIds`, applications, grants and role assignments within a member's default rights (doctor tries each); the portal link blades (`mod.rs`); that `alternativeNames` matches an ARM id in any case and holds it after an `isExplicit=` entry (`mod.rs`, `serviceprincipal/get.rs`); 15 as the cap on `in` (`principal/list.rs`, `role/list.rs`); that `ownedObjects` and `transitiveMemberOf` take a derived type's properties in `$select` (`group/list.rs`, `app/list.rs`, `role/list.rs`); that `$count` lags (`group/get.rs`); beta sign-ins: that `servicePrincipalId` is the object id, whether `status/errorCode ne 0` filters (failures are kept client-side from one 1,000-row page today), that `appliedConditionalAccessPolicies` is omitted without Policy.Read.All (`signin/list.rs`); that a credential change in the audit log names key ids and display names, never a value (`audit/list.rs`); the logs' throttle (five calls in ten seconds per app per tenant, every az user being the same app)
- [ ] When asked: writes (group member add and remove, credential delete; the token allows them, which is why they wait); PIM-eligible role assignments; administrative units, devices, national clouds, enterprise apps' SAML certificates; `entra signin list` on v1.0 once it lists service principals' and identities' sign-ins

## Phase 5: consolidate

- [ ] After the live runs, the TUIs depend on agent-cli's domain crates and delete their copies and their CLIs; each TUI's tests pass on the shared crates
- Not ported, on purpose: ticket-tui `sync` (no local database), `status` (a TUI status-bar line) and `agent launch|prompt|list` (Herdr orchestration; `ado workitem get` gives an agent the content); az-tui `setup --write` (agents get `aks cluster list`, and the README shows the TOML); sql-bench's replay flags and `--format table|csv` (output is JSON only)

## Trials

- [ ] A raw-tools trial (kubectl, az, curl and pup against agent-cli on the cross-domain tasks): its tasks include the four cluster traces `crates/cli/tests/world_cross.rs` walks (private DNS, Multi-Attach, pending pods, workload identity 403; `fixtures/world/README.md`). A domain earns its place if agent-cli wins at least half its tasks, by 20% of tokens or on correctness; `arm` and `net` are on trial as domains
- [ ] Development trials: tokens fell 29% on one run per task but 18% on two, short of the 25% target (`docs/trials/dev-after-2026-09-30.md`). Each call carries about 27k tokens of fixed context, so cut calls: a scaffold that fills in more of a routine command, a check that builds once for both feature sets. Next round: three runs per task

## Small follow-ups

- [ ] Time arithmetic in `--since`/`--until` (`T-2h`): Haiku tried `--until 2d-23h` and `--since 2d-2h` in the chains-2 trial (each an exit 2 it recovered from)
- [ ] The skill's `paths` hide it until a matching file is touched, so agents found it through `AGENTS.md` and read it with `cat`; drop `paths` if it should be offered from the first turn

## ADO query efficiency (after the work rollout)

Projects of thousands of work items and hundreds of people. Measure first, then cut.

- [ ] Count the requests and bytes each command sends (a stderr note under a debug env var through `ctx.env`), then run the pm skill's sweep and the trial tasks against the work org to find the expensive commands
- [ ] `workitem list` asks WIQL for `$top=20000` ids only to print `[50 of N]`: a broad query returns thousands of ids every call. Try `$top=limit+1` with `[50 of 51+]`, or keep the exact count only when it is cheap
- [ ] `workitemsbatch` asks for only the fields the row prints (check `fields` against `--fields`), 200 ids a call
- [ ] Cache identity lookups (`@me`, `--assignee` names) like types and sprints, so a sweep doesn't search identities on every call
- [ ] The fan-outs in `activity list` (updates for 50 work items, commits from 20 repositories) and `person list`: batch them, or give them a `--since` cut-off
- [ ] Rate limits: ADO meters TSTUs (200 per user per sliding 5 minutes); record `X-RateLimit-Remaining` and `X-RateLimit-Delay` in the debug note so a sweep that comes near the limit shows up before it is throttled
- [ ] Incremental reads for the pm sweep: `[System.ChangedDate] > last sweep` plus the cached rows, if sweeps get repeated within a day

## Built only when asked

- [ ] `ado pr list --build succeeded|failed|running|none`: the PR search carries no build status, so it costs one policy-evaluations read per PR (what `pr get` does); cap it
- [ ] The ceilings ado's work item commands left (each a `ponytail:` comment): `sprint get` counts Monday to Friday (read teamsettings' working days); `workitem update --above/--below` ranks with parentId 0 (a parent's id for an item nested under it); `person list` reads one page of 1,000 members a team; `attachment get` reads at most core's 32 MiB of an answer, ADO keeps 60 MB (stream to `--output`); `activity list` reads updates for at most 50 work items and commits from at most 20 repositories
- [ ] `airflow run cancel`: one destructive `PATCH` of the run's state to failed
- [ ] An Airflow 2 session-cookie cache (0600, keyed by `base_url` and user), if a work instance accepts only session auth: today each command signs in through the form, and the webserver allows about five sign-ins per 40 s
- [ ] Oracle by SID (`sid` beside `service`, `(CONNECT_DATA=(SID=…))`), if a work database is reached only by SID
- [ ] A live test, off by default: `AGENT_CLI_TEST_DATADOG=1` running every dd read with `--limit 1` against the sandbox in `docs/first-live-run.md`
- [ ] Commands that run a whole chain (an `airflow task triage`: the error, its source lines, the upstream XCom and `repo_file` in one row), for a flow a trial shows still costs Haiku 15 or more calls with the notes in place
- [ ] An output mode of one bare value per line, for `for id in $(…)` without `jq`, if scripts become a goal: a global flag (not `--lines`, a `--tail` synonym) when `--fields` names one scalar
- [ ] A `[datadog]` service-to-repository map, if the live org lacks the source code integration and a dd `at` without git tags (`PATH:LINE`) proves too little
